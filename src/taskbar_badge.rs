//! Desktop unread-chat badge. Never attached to demos or UI benchmarks.
use crate::{AppWindow, messaging::Snapshot};
use tokio::sync::watch;

pub async fn run(ui: slint::Weak<AppWindow>, snapshots: watch::Receiver<Snapshot>) {
    #[cfg(target_os = "linux")]
    {
        drop(ui);
        // A session connection must remain alive: launchers may remove its badge
        // when the sender disconnects. zbus uses our existing Tokio runtime.
        match tokio::time::timeout(linux::IO_TIMEOUT, zbus::Connection::session()).await {
            Ok(Ok(connection)) => linux::publish(snapshots, connection).await,
            _ => eprintln!("Heya taskbar badge: session bus unavailable"),
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        let shown = AtomicU64::new(u64::MAX);
        crate::live_presentation::present_latest(ui.clone(), snapshots, move |_, snapshot| {
            if let Some(view) = &snapshot.view {
                let count = u64::from(view.unread_chats);
                if shown.swap(count, Ordering::Relaxed) != count {
                    crate::macos::set_dock_badge(view.unread_chats);
                }
            }
        })
        .await;
        let _ = ui.upgrade_in_event_loop(|_| crate::macos::set_dock_badge(0));
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = (ui, snapshots);
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{collections::HashMap, time::Duration};
    use zbus::{Connection, zvariant::Value};

    pub(super) const IO_TIMEOUT: Duration = Duration::from_secs(2);
    const PATH: &str = "/com/canonical/unity/launcherentry/heya";
    const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
    const URI: &str = "application://org.heya.Heya.desktop";

    async fn emit(connection: &Connection, count: u32) -> zbus::Result<()> {
        let mut properties = HashMap::from([("count-visible", Value::from(count > 0))]);
        if count > 0 {
            properties.insert("count", Value::from(i64::from(count)));
        }
        connection
            .emit_signal(None::<&str>, PATH, INTERFACE, "Update", &(URI, properties))
            .await
    }

    pub(super) async fn publish(mut snapshots: watch::Receiver<Snapshot>, connection: Connection) {
        let mut shown = None;
        loop {
            // Latest-value snapshots coalesce bursts. A save reply or failed
            // archive query has no view and must not clear the previous badge.
            let count = snapshots
                .borrow_and_update()
                .view
                .as_ref()
                .map(|v| v.unread_chats.min(9999));
            if let Some(count) = count
                && shown != Some(count)
            {
                if !matches!(
                    tokio::time::timeout(IO_TIMEOUT, emit(&connection, count)).await,
                    Ok(Ok(()))
                ) {
                    eprintln!("Heya taskbar badge: update unavailable");
                    break;
                }
                shown = Some(count);
            }
            tokio::select! {
                result = snapshots.changed() => if result.is_err() { break; },
                _ = connection.closed() => break,
            }
        }
        // Closing the main window closes the snapshot stream. Clear the badge
        // and explicitly release its connection; no worker thread or timer stays.
        let _ = tokio::time::timeout(IO_TIMEOUT, async {
            if shown.is_some_and(|count| count > 0) {
                let _ = emit(&connection, 0).await;
            }
            let _ = connection.close().await;
        })
        .await;
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "taskbar_badge_tests.rs"]
mod tests;
