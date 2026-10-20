//! Real launcher signals on a disposable private bus; never the owner's taskbar.
use super::*;
use futures_util::StreamExt;
use std::{collections::HashMap, process::Stdio, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, BufReader};
use zbus::{Connection, MatchRule, MessageStream, message::Type, zvariant::OwnedValue};

fn snapshot(count: u32) -> Snapshot {
    Snapshot {
        view: Some(Arc::new(crate::archive::View {
            unread_chats: count,
            archived_count: 0,
            archived_view: false,
            chats: Arc::from([]),
            conversation: None,
            can_send: false,
            history_failed: false,
            phone_more: false,
        })),
        ..Snapshot::empty()
    }
}

async fn bus() -> (
    tempfile::TempDir,
    tokio::process::Child,
    Connection,
    Connection,
) {
    let temp = tempfile::tempdir().unwrap();
    let mut daemon = tokio::process::Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address"])
        .arg(format!(
            "--address=unix:path={}",
            temp.path().join("bus").display()
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap())
        .read_line(&mut address)
        .await
        .unwrap();
    let observer = zbus::connection::Builder::address(address.trim())
        .unwrap()
        .build()
        .await
        .unwrap();
    let publisher = zbus::connection::Builder::address(address.trim())
        .unwrap()
        .build()
        .await
        .unwrap();
    (temp, daemon, observer, publisher)
}

async fn signals(connection: &Connection) -> MessageStream {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .path("/com/canonical/unity/launcherentry/heya")
        .unwrap()
        .interface("com.canonical.Unity.LauncherEntry")
        .unwrap()
        .member("Update")
        .unwrap()
        .build();
    MessageStream::for_match_rule(rule, connection, Some(16))
        .await
        .unwrap()
}

async fn count(stream: &mut MessageStream) -> Option<i64> {
    let message = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (uri, properties): (String, HashMap<String, OwnedValue>) =
        message.body().deserialize().unwrap();
    assert_eq!(uri, "application://org.heya.Heya.desktop");
    let visible = bool::try_from(properties.get("count-visible").unwrap()).unwrap();
    assert_eq!(properties.len(), if visible { 2 } else { 1 });
    visible.then(|| i64::try_from(properties.get("count").unwrap()).unwrap())
}

async fn unchanged(stream: &mut MessageStream) {
    assert!(
        tokio::time::timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn launcher_count_coalesces_preserves_reply_only_views_and_clears_on_close() {
    let (_temp, mut daemon, observer, publisher) = bus().await;
    let name = publisher.unique_name().unwrap().to_owned();
    let mut signals = signals(&observer).await;
    let (updates, snapshots) = watch::channel(Snapshot::empty());
    for n in 1..=100 {
        updates.send_replace(snapshot(n));
    }
    let worker = tokio::spawn(super::linux::publish(snapshots, publisher));
    assert_eq!(count(&mut signals).await, Some(100));
    for _ in 0..100 {
        updates.send_replace(snapshot(100));
    }
    unchanged(&mut signals).await;
    updates.send_replace(Snapshot::empty());
    unchanged(&mut signals).await;
    updates.send_replace(Snapshot {
        notice: "Synthetic failure".into(),
        ..Snapshot::empty()
    });
    unchanged(&mut signals).await;
    updates.send_replace(snapshot(2));
    assert_eq!(count(&mut signals).await, Some(2));
    updates.send_replace(snapshot(0));
    assert_eq!(count(&mut signals).await, None);
    updates.send_replace(snapshot(100_000));
    assert_eq!(count(&mut signals).await, Some(9999));
    updates.send_replace(snapshot(200_000));
    unchanged(&mut signals).await;
    updates.send_replace(snapshot(1));
    assert_eq!(count(&mut signals).await, Some(1));
    drop(updates);
    assert_eq!(count(&mut signals).await, None);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    let proxy = zbus::fdo::DBusProxy::new(&observer).await.unwrap();
    assert!(!proxy.name_has_owner(name.into()).await.unwrap());
    drop(signals);
    observer.close().await.unwrap();
    daemon.kill().await.unwrap();
    daemon.wait().await.unwrap();
}

#[tokio::test]
async fn losing_the_bus_stops_the_badge_without_waiting_for_ui_or_account_updates() {
    let (_temp, mut daemon, observer, publisher) = bus().await;
    let mut signals = signals(&observer).await;
    let (_updates, snapshots) = watch::channel(snapshot(3));
    let worker = tokio::spawn(super::linux::publish(snapshots, publisher));
    assert_eq!(count(&mut signals).await, Some(3));
    daemon.kill().await.unwrap();
    daemon.wait().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
}
