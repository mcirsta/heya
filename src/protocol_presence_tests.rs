use super::*;
use crate::archive::tests::{LID, PN, fixture};
use tokio::sync::{mpsc, oneshot};
use whatsapp_rust::types::events::PresenceUpdate;

const OTHER: &str = "15550000002@s.whatsapp.net";

fn event(events: &Events, jid: &str, online: bool, seen: Option<i64>) {
    events.handle_event(Arc::new(Event::Presence(
        PresenceUpdate::builder()
            .from(jid.parse().unwrap())
            .status(if online {
                PresenceStatus::Available
            } else {
                PresenceStatus::Unavailable
            })
            .maybe_last_seen(seen.and_then(|s| chrono::DateTime::from_timestamp(s, 0)))
            .build(),
    )));
}

#[test]
fn presence_events_filter_aliases_coalesce_bursts_and_clear_private_last_seen() {
    let events = Events::default();
    events.select(Some(Target {
        chat: 1,
        jid: PN.into(),
        addresses: vec![PN.into(), LID.into()],
    }));
    let mut rx = events.latest.subscribe();
    event(&events, OTHER, true, None);
    assert!(!rx.has_changed().unwrap());
    event(&events, LID, true, None);
    assert!(rx.borrow_and_update().online);
    for _ in 0..2000 {
        event(&events, PN, true, None);
    }
    assert!(
        !rx.has_changed().unwrap(),
        "duplicates add no work to the UI queue"
    );
    event(&events, PN, false, Some(1_700_000_000));
    assert_eq!(rx.borrow().last_seen, Some(1_700_000_000));
    event(&events, PN, false, None);
    assert!(!rx.borrow().online);
    assert_eq!(rx.borrow().last_seen, None);
    events.select(None);
    rx.borrow_and_update();
    event(&events, PN, true, None);
    assert!(!rx.has_changed().unwrap());
    assert_eq!(rx.borrow().chat, 0);
}

#[tokio::test]
async fn presence_targets_exclude_groups_self_and_unsupported_addresses() {
    let (_temp, db) = fixture();
    let peer = db.create_chat(PN.into()).await.unwrap();
    db.save_contacts(vec![], vec![(LID.into(), PN.into())], false)
        .await
        .unwrap();
    let target = db.presence_target(peer).await.unwrap().unwrap();
    assert_eq!(target.jid, PN);
    assert!(target.addresses.contains(&LID.into()));
    assert!(db.presence_target(0).await.unwrap().is_none());
    assert!(db.presence_target(99_999).await.unwrap().is_none());
    let group = db.create_chat("10000000001@g.us".into()).await.unwrap();
    assert!(db.presence_target(group).await.unwrap().is_none());
    db.bind_account(PN.into(), "Synthetic self".into())
        .await
        .unwrap();
    assert!(db.presence_target(peer).await.unwrap().is_none());
    let read_only = db.create_chat(OTHER.into()).await.unwrap();
    db.call(move |c| {
        c.execute("UPDATE chats SET read_only=1 WHERE id=?1", [read_only])?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(db.presence_target(read_only).await.unwrap().is_none());
}

struct Call {
    subscribe: bool,
    jid: String,
    done: oneshot::Sender<bool>,
}
struct Fake(mpsc::UnboundedSender<Call>);
impl Fake {
    async fn call(&self, jid: &str, subscribe: bool) -> bool {
        let (done, answer) = oneshot::channel();
        self.0
            .send(Call {
                subscribe,
                jid: jid.into(),
                done,
            })
            .unwrap();
        answer.await.unwrap_or(false)
    }
}
#[async_trait::async_trait]
impl Transport for Fake {
    async fn subscribe(&self, jid: &str) -> bool {
        self.call(jid, true).await
    }
    async fn unsubscribe(&self, jid: &str) -> bool {
        self.call(jid, false).await
    }
}
async fn call(
    rx: &mut mpsc::UnboundedReceiver<Call>,
    subscribe: bool,
    jid: &str,
) -> oneshot::Sender<bool> {
    let call = timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.subscribe, subscribe);
    assert_eq!(call.jid, jid);
    call.done
}
async fn snapshot(rx: &mut watch::Receiver<Snapshot>, chat: i32, label: &str) {
    timeout(Duration::from_secs(3), async {
        loop {
            let next = rx.borrow_and_update().clone();
            if next.chat == chat && next.label == label {
                break;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn presence_worker_switches_one_peer_at_a_time_and_clears_on_reconnect_and_shutdown() {
    let (_temp, db) = fixture();
    let a = db.create_chat(PN.into()).await.unwrap();
    let b = db.create_chat(OTHER.into()).await.unwrap();
    let (selected, selected_rx) = watch::channel(a);
    let (online, online_rx) = watch::channel(false);
    let (stop, stopped) = watch::channel(false);
    let (snapshots, mut rx) = watch::channel(Snapshot::default());
    let (calls, mut calls_rx) = mpsc::unbounded_channel();
    let events = Events::default();
    let task = tokio::spawn(worker(
        db,
        Fake(calls),
        events.clone(),
        Channels {
            selected: selected_rx,
            snapshots,
        },
        online_rx,
        stopped,
        Timing::default(),
    ));
    assert!(
        timeout(Duration::from_millis(30), calls_rx.recv())
            .await
            .is_err()
    );
    online.send_replace(true);
    let subscribed = call(&mut calls_rx, true, PN).await;
    event(&events, PN, true, None); // Even an event arriving before subscribe() returns.
    subscribed.send(true).unwrap();
    snapshot(&mut rx, a, "online").await;
    selected.send_replace(b);
    let removed = call(&mut calls_rx, false, PN).await;
    snapshot(&mut rx, 0, "").await;
    event(&events, PN, true, None);
    assert_eq!(rx.borrow().chat, 0);
    removed.send(true).unwrap();
    let added = call(&mut calls_rx, true, OTHER).await;
    event(&events, OTHER, true, None);
    added.send(true).unwrap();
    snapshot(&mut rx, b, "online").await;
    online.send_replace(false);
    snapshot(&mut rx, 0, "").await;
    online.send_replace(true);
    call(&mut calls_rx, false, OTHER).await.send(true).unwrap();
    let added = call(&mut calls_rx, true, OTHER).await;
    assert!(
        rx.borrow().label.is_empty(),
        "old online status must not survive reconnect"
    );
    event(&events, OTHER, true, None);
    added.send(true).unwrap();
    snapshot(&mut rx, b, "online").await;
    selected.send_replace(0);
    let pending = call(&mut calls_rx, false, OTHER).await;
    stop.send_replace(true);
    timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(pending.is_closed());
    assert!(rx.borrow().label.is_empty());
    event(&events, OTHER, true, None);
    assert_eq!(events.latest.borrow().chat, 0);
}

#[tokio::test]
async fn presence_worker_cleans_cancelled_or_timed_out_subscriptions_before_adding_another() {
    let (_temp, db) = fixture();
    let a = db.create_chat(PN.into()).await.unwrap();
    let b = db.create_chat(OTHER.into()).await.unwrap();
    let (selected, selected_rx) = watch::channel(a);
    let (_online, online_rx) = watch::channel(true);
    let (stop, stopped) = watch::channel(false);
    let (snapshots, rx) = watch::channel(Snapshot::default());
    let (calls, mut calls_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(worker(
        db,
        Fake(calls),
        Events::default(),
        Channels {
            selected: selected_rx,
            snapshots,
        },
        online_rx,
        stopped,
        Timing {
            request: Duration::from_millis(100),
            retry: Duration::from_millis(120),
        },
    ));
    let late = call(&mut calls_rx, true, PN).await;
    selected.send_replace(b);
    call(&mut calls_rx, false, PN).await.send(true).unwrap();
    assert!(late.is_closed());
    let timed_out = call(&mut calls_rx, true, OTHER).await;
    // Wait for the cancellation of the request itself before checking backoff.
    let mut timed_out = timed_out;
    timeout(Duration::from_secs(3), timed_out.closed())
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_millis(40), calls_rx.recv())
            .await
            .is_err()
    );
    call(&mut calls_rx, false, OTHER).await.send(false).unwrap();
    assert!(
        timeout(Duration::from_millis(40), calls_rx.recv())
            .await
            .is_err()
    );
    call(&mut calls_rx, false, OTHER).await.send(true).unwrap();
    let pending = call(&mut calls_rx, true, OTHER).await;
    assert!(rx.borrow().label.is_empty());
    stop.send_replace(true);
    timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(pending.is_closed());
}
