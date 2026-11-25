//! Unified Linux desktop system integration for NEON DE.
//!
//! Coordinates D-Bus and PipeWire services cleanly off the UI thread:
//! - NetworkManager (`org.freedesktop.NetworkManager`)
//! - PipeWire / WirePlumber (`wpctl`)
//! - BlueZ (`org.bluez`)
//! - UPower & PowerProfiles (`org.freedesktop.UPower`, `org.freedesktop.UPower.PowerProfiles`)
//! - Freedesktop Notifications v1.2 (`org.freedesktop.Notifications` / `org.neonde.Notifications`)
//! - PolicyKit1 (`org.freedesktop.PolicyKit1.AuthenticationAgent`)
//! - XDG Desktop Portals (`org.freedesktop.portal.Desktop`)

pub mod apps;
pub mod audio;
pub mod bluetooth;
pub mod datetime;
pub mod media;
pub mod network;
pub mod notifications;
pub mod polkit;
pub mod portals;
pub mod storage;
pub mod tray;
pub mod updates;
pub mod users;

use std::{sync::mpsc, thread, time::Duration};

use serde::{Deserialize, Serialize};
use tracing::warn;
use zbus::blocking::Connection;

use crate::{
    audio::AudioStatus, bluetooth::BluetoothStatus, media::MediaStatus, network::NetworkStatus,
    notifications::NotificationServiceStatus, polkit::PolkitStatus, portals::PortalStatus,
    power::PowerStatus,
};

pub mod power;

/// Combined snapshot of all live Linux desktop subsystems.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SystemSnapshot {
    pub network: NetworkStatus,
    pub audio: AudioStatus,
    pub bluetooth: BluetoothStatus,
    pub power: PowerStatus,
    pub media: MediaStatus,
    pub notifications: NotificationServiceStatus,
    pub polkit: PolkitStatus,
    pub portals: PortalStatus,
}

/// Asynchronous control actions dispatched from UI threads to the background system worker.
#[derive(Debug, Clone)]
pub enum SystemAction {
    RefreshNow,
    SetWifiEnabled(bool),
    SetBluetoothPowered(bool),
    SetBluetoothDeviceConnected { object_path: String, connect: bool },
    SetSinkVolume(f32),
    SetSinkMute(bool),
    ToggleSinkMute,
    ToggleSourceMute,
    SetDefaultSink(u32),
    SetPowerProfile(String),
    SetBrightness(u32),
    MediaCommand { bus_name: Option<String>, method: String },
    InvokePowerAction(String),
    LogOut,
    SendTestNotification { bus_name: String },
}

/// Queries a full synchronous snapshot across all Linux desktop subsystems.
pub fn query_system_snapshot() -> SystemSnapshot {
    let sys_conn = Connection::system().ok();

    let network = sys_conn
        .as_ref()
        .map(network::query_network_with_connection)
        .unwrap_or_default();
    let bluetooth = sys_conn
        .as_ref()
        .map(bluetooth::query_bluetooth_with_connection)
        .unwrap_or_default();
    let power = sys_conn
        .as_ref()
        .map(power::query_power_with_connection)
        .unwrap_or_default();
    let audio = audio::query_audio_status();
    let media = media::query_media_status();
    let notifications = notifications::query_notification_service_status();
    let polkit = polkit::query_polkit_status();
    let portals = portals::query_portal_status();

    SystemSnapshot {
        network,
        audio,
        bluetooth,
        power,
        media,
        notifications,
        polkit,
        portals,
    }
}

/// Executes a single [`SystemAction`] synchronously.
pub fn execute_system_action(action: &SystemAction) -> Result<(), String> {
    match action {
        SystemAction::RefreshNow => Ok(()),
        SystemAction::SetWifiEnabled(enabled) => network::set_wifi_enabled(*enabled),
        SystemAction::SetBluetoothPowered(powered) => bluetooth::set_bluetooth_powered(*powered),
        SystemAction::SetBluetoothDeviceConnected {
            object_path,
            connect,
        } => bluetooth::set_device_connected(object_path, *connect),
        SystemAction::SetSinkVolume(vol) => audio::set_sink_volume(*vol),
        SystemAction::SetSinkMute(muted) => audio::set_sink_mute(*muted),
        SystemAction::ToggleSinkMute => audio::toggle_sink_mute(),
        SystemAction::ToggleSourceMute => audio::toggle_source_mute(),
        SystemAction::SetDefaultSink(id) => audio::set_default_sink(*id),
        SystemAction::SetPowerProfile(profile) => power::set_power_profile(profile),
        SystemAction::SetBrightness(pct) => power::set_backlight_percent(*pct),
        SystemAction::MediaCommand { bus_name, method } => {
            media::invoke_media_command(bus_name.as_deref(), method)
        }
        SystemAction::InvokePowerAction(method) => power::invoke_logind_action(method),
        SystemAction::LogOut => power::terminate_current_session(),
        SystemAction::SendTestNotification { bus_name } => notifications::send_dbus_notification(
            bus_name,
            "NEON DE",
            "NEON DE System Integration",
            "Desktop notifications, PipeWire audio, NetworkManager, BlueZ, and UPower are active.",
            notifications::NotificationUrgency::Normal,
            4500,
        )
        .map(|_| ()),
    }
}

/// Non-blocking handle for GTK4 UI apps (`neon-shell`, `neon-settings`) to consume
/// periodic [`SystemSnapshot`] updates and send [`SystemAction`] commands off the UI thread.
#[derive(Clone)]
pub struct SystemActionSender {
    tx: mpsc::Sender<SystemAction>,
}

impl SystemActionSender {
    pub fn send(&self, action: SystemAction) {
        let _ = self.tx.send(action);
    }
}

/// Spawns a lightweight background worker thread that polls system services every
/// `poll_interval` and executes incoming [`SystemAction`] commands immediately.
pub fn start_system_monitor(
    poll_interval: Duration,
) -> (SystemActionSender, mpsc::Receiver<SystemSnapshot>) {
    let (action_tx, action_rx) = mpsc::channel::<SystemAction>();
    let (snap_tx, snap_rx) = mpsc::channel::<SystemSnapshot>();

    thread::Builder::new()
        .name("neon-system-monitor".to_string())
        .spawn(move || {
            let mut last_snapshot = query_system_snapshot();
            if snap_tx.send(last_snapshot.clone()).is_err() {
                return;
            }

            loop {
                match action_rx.recv_timeout(poll_interval) {
                    Ok(action) => {
                        if let Err(err) = execute_system_action(&action) {
                            warn!(?action, %err, "System action failed");
                        }
                        // Drain any burst of rapid slider actions before re-querying.
                        let mut latest_vol: Option<f32> = None;
                        let mut latest_brightness: Option<u32> = None;
                        while let Ok(next) = action_rx.try_recv() {
                            match next {
                                SystemAction::SetSinkVolume(v) => latest_vol = Some(v),
                                SystemAction::SetBrightness(b) => latest_brightness = Some(b),
                                other => {
                                    if let Err(err) = execute_system_action(&other) {
                                        warn!(?other, %err, "Queued system action failed");
                                    }
                                }
                            }
                        }
                        if let Some(v) = latest_vol
                            && let Err(err) = audio::set_sink_volume(v)
                        {
                            warn!(%err, "Coalesced sink volume action failed");
                        }
                        if let Some(b) = latest_brightness
                            && let Err(err) = power::set_backlight_percent(b)
                        {
                            warn!(%err, "Coalesced display brightness action failed");
                        }
                        let snap = query_system_snapshot();
                        last_snapshot = snap.clone();
                        if snap_tx.send(snap).is_err() {
                            return;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let snap = query_system_snapshot();
                        if snap != last_snapshot {
                            last_snapshot = snap.clone();
                            if snap_tx.send(snap).is_err() {
                                return;
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .expect("failed to spawn neon-system-monitor thread");

    (SystemActionSender { tx: action_tx }, snap_rx)
}
