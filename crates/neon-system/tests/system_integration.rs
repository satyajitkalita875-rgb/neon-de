use std::sync::Arc;

use neon_system::{
    notifications::{
        NOTIFICATIONS_IFACE, NOTIFICATIONS_PATH, NotificationEvent, NotificationStore,
        NotificationUrgency, send_dbus_notification, start_notification_daemon,
    },
    query_system_snapshot,
};
use zbus::blocking::{Connection, Proxy};

#[test]
fn dbus_notifications_v1_2_server_roundtrip() {
    let store = Arc::new(NotificationStore::new());
    let rx = store.subscribe();
    let test_bus = format!("org.neonde.NotificationsTest{}", std::process::id());

    let handle = match start_notification_daemon(Arc::clone(&store), Some(&test_bus)) {
        Ok(h) => h,
        Err(err) => {
            // Skip cleanly only if no session bus exists in a stripped container.
            eprintln!("Skipping D-Bus session bus test (no session bus): {err}");
            return;
        }
    };
    assert_eq!(handle.bus_name, test_bus);

    let conn = Connection::session().expect("session bus must connect");
    let proxy = Proxy::new(
        &conn,
        test_bus.as_str(),
        NOTIFICATIONS_PATH,
        NOTIFICATIONS_IFACE,
    )
    .expect("proxy creation must succeed");

    let caps: Vec<String> = proxy
        .call("GetCapabilities", &())
        .expect("GetCapabilities must succeed");
    assert!(caps.iter().any(|c| c == "body"));
    assert!(caps.iter().any(|c| c == "persistence"));

    let (name, vendor, _ver, spec): (String, String, String, String) = proxy
        .call("GetServerInformation", &())
        .expect("GetServerInformation must succeed");
    assert_eq!(name, "NEON DE Notifications");
    assert_eq!(vendor, "NEON DE");
    assert_eq!(spec, "1.2");

    let id = send_dbus_notification(
        &test_bus,
        "NEON Test",
        "Milestone 7",
        "Real D-Bus Freedesktop Notifications v1.2 works!",
        NotificationUrgency::Critical,
        4000,
    )
    .expect("Notify call must succeed");
    assert!(id >= 1);

    let event = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    match event {
        NotificationEvent::Added(n) => {
            assert_eq!(n.id, id);
            assert_eq!(n.app_name, "NEON Test");
            assert_eq!(n.summary, "Milestone 7");
            assert_eq!(n.urgency, NotificationUrgency::Critical);
        }
        other => panic!("expected Added event, got {other:?}"),
    }

    proxy
        .call::<_, _, ()>("CloseNotification", &(id,))
        .expect("CloseNotification must succeed");
    let close_evt = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    assert_eq!(close_evt, NotificationEvent::Closed { id, reason: 3 });
    assert_eq!(store.active_count(), 0);
}

#[test]
fn query_system_snapshot_completes_cleanly() {
    let snap = query_system_snapshot();
    // Should never panic regardless of which daemons are present; log live probe values.
    println!(
        "Live SystemSnapshot: nm={} (conn={:?}, label='{}'), audio={} (label='{}', sinks={}), bt={} (powered={}, devs={}), power={} (label='{}', profile={:?}), polkit={}, portals={}",
        snap.network.available,
        snap.network.connectivity,
        snap.network.panel_label(),
        snap.audio.available,
        snap.audio.panel_label(),
        snap.audio.sinks.len(),
        snap.bluetooth.available,
        snap.bluetooth.powered,
        snap.bluetooth.devices.len(),
        snap.power.available,
        snap.power.panel_label(),
        snap.power.active_profile,
        snap.polkit.daemon_available,
        snap.portals.desktop_portal_running,
    );
    assert!(!snap.polkit.agent_object_path.is_empty());
}
