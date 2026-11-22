# NEON DE — Linux System Integration (`neon-system`)

Milestone 7 integrates NEON DE cleanly with standard Linux desktop plumbing over D-Bus (`zbus`) and PipeWire/WirePlumber, without reinventing OS daemons or disrupting host sessions during nested development.

## 1. Integrated Linux Subsystems

| Subsystem | Protocol / Interface | Capabilities |
|---|---|---|
| **Networking** | `org.freedesktop.NetworkManager` (System D-Bus) | Connectivity state, primary connection type (Wi-Fi / Wired / VPN), active SSID, Wi-Fi signal strength, live `WirelessEnabled` toggle |
| **Audio** | PipeWire + WirePlumber (`wpctl`) | Default sink & microphone volume (0–150%), mute state, sink discovery, live volume slider, mute toggle, default sink switching |
| **Bluetooth** | `org.bluez` (`ObjectManager`, `Adapter1`, `Device1`) | Adapter discovery, live power toggle, paired & connected device enumeration, live Connect/Disconnect |
| **Power & Battery** | `org.freedesktop.UPower`, `org.freedesktop.UPower.PowerProfiles`, `org.freedesktop.login1` | Battery percentage, charging/discharging state, time estimates, live Power Profile switching (`power-saver`, `balanced`, `performance`), suspend/reboot/poweroff capability queries |
| **Notifications** | `org.freedesktop.Notifications` v1.2 (Session D-Bus) | `GetCapabilities`, `GetServerInformation`, `Notify`, `CloseNotification`, bounded history, urgency (`Low`/`Normal`/`Critical`), Do-Not-Disturb, live GTK4 layer-shell toast stack |
| **PolicyKit** | `org.freedesktop.PolicyKit1.AuthenticationAgent` | System bus availability check, compliant `BeginAuthentication` / `CancelAuthentication` D-Bus object interface (`/org/neonde/PolicyKit1/AuthenticationAgent`) |
| **XDG Portals** | `org.freedesktop.portal.Desktop`, `neon-de-portals.conf` | Live portal & documents service discovery, installed backend enumeration (`gtk`, `gnome-keyring`), `XDG_CURRENT_DESKTOP=NEON-DE` export, and packaged portal config template in `data/xdg-desktop-portal/neon-de-portals.conf` |

## 2. Host-Safety Invariants (Nested Development Mode)

- **Notifications**: Before requesting `org.freedesktop.Notifications`, `neon-system` queries `NameHasOwner`. In a standalone NEON DE session where the name is unowned, NEON DE claims `org.freedesktop.Notifications`. In a nested session where the host desktop already owns it, NEON DE **never** replaces the host daemon and automatically binds `org.neonde.Notifications` (same `/org/freedesktop/Notifications` object path and `org.freedesktop.Notifications` interface).
- **PolicyKit**: `neon-system` exports the `AuthenticationAgent` object without stealing a host desktop's active session registration unless explicitly invoked in a standalone NEON DE session.
- **Non-blocking UI**: `start_system_monitor()` runs all D-Bus and PipeWire calls on a dedicated worker thread (`neon-system-monitor`) and coalesces rapid volume slider updates so `neon-shell` renders at full frame rate without D-Bus stalls.

## 3. UI Surfaces

- **Top Panel (`neon-shell`)**: Displays live Network, PipeWire Volume (click to mute/unmute), UPower Battery, and Control Center (`System`) pills.
- **Control Center (`neon-shell`)**: Top-right `Layer::Overlay` popover with live PipeWire volume slider, Wi-Fi & Bluetooth switches, paired Bluetooth device buttons, Power Profile pills, Do-Not-Disturb toggle, and Notification History.
- **Notification Toasts (`neon-shell`)**: Top-right `Layer::Overlay` toast cards with auto-expiration and urgency borders (automatically suppressed while Control Center is open or Do-Not-Disturb is active).
- **Settings App (`neon-settings`)**: The *System* page provides live Audio, Network, Bluetooth, Power Profile, Notification test, PolicyKit1, and XDG Portal controls and diagnostics.
