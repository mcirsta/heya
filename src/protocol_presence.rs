//! Selected-peer subscription and a single coalesced event slot. No presence DB.
use crate::{
    archive::{self, Archive},
    presence::{Channels, Snapshot, Target, Update},
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::watch,
    time::{Instant, sleep_until, timeout},
};
use whatsapp_rust::{
    Client, PresenceStatus,
    prelude::{Event, EventHandler, EventInterest, EventKind},
};

#[derive(Clone)]
pub struct Events {
    target: Arc<Mutex<Option<Target>>>,
    latest: watch::Sender<Update>,
}
impl Default for Events {
    fn default() -> Self {
        Self {
            target: Arc::new(Mutex::new(None)),
            latest: watch::channel(Update::default()).0,
        }
    }
}
impl Events {
    fn select(&self, target: Option<Target>) {
        let mut slot = self.target.lock().unwrap_or_else(|e| e.into_inner());
        *slot = target;
        self.latest.send_replace(Update::default());
    }
}
impl EventHandler for Events {
    fn interest(&self) -> EventInterest {
        EventInterest::of(&[EventKind::Presence])
    }
    fn handle_event(&self, event: Arc<Event>) {
        let Event::Presence(event) = event.as_ref() else {
            return;
        };
        let slot = self.target.lock().unwrap_or_else(|e| e.into_inner());
        let Some(target) = slot.as_ref() else {
            return;
        };
        let address = crate::protocol_messages::address(&event.from);
        if !target.addresses.contains(&address) {
            return;
        }
        let online = event.status == PresenceStatus::Available;
        let next = Update {
            chat: target.chat,
            online,
            last_seen: (!online)
                .then_some(event.last_seen)
                .flatten()
                .map(|t| t.timestamp()),
        };
        // Unknown/hidden last-seen clears the old value; never infer it from
        // a message or from the local clock when an offline event arrives.
        self.latest.send_if_modified(|old| {
            if *old == next {
                false
            } else {
                *old = next;
                true
            }
        });
    }
}

#[async_trait::async_trait]
trait Transport: Send + Sync {
    async fn subscribe(&self, jid: &str) -> bool;
    async fn unsubscribe(&self, jid: &str) -> bool;
}
struct WhatsApp(Arc<Client>);
#[async_trait::async_trait]
impl Transport for WhatsApp {
    async fn subscribe(&self, jid: &str) -> bool {
        let Ok(jid) = jid.parse::<whatsapp_rust::wacore_binary::Jid>() else {
            return false;
        };
        if self
            .0
            .wait_for_session_ready(Duration::from_secs(8))
            .await
            .is_err()
        {
            return false;
        }
        self.0.presence().subscribe(jid).await.is_ok()
    }
    async fn unsubscribe(&self, jid: &str) -> bool {
        let Ok(jid) = jid.parse::<whatsapp_rust::wacore_binary::Jid>() else {
            return false;
        };
        self.0.presence().unsubscribe(&jid).await.is_ok()
    }
}

pub async fn run(
    archive: Archive,
    client: Arc<Client>,
    events: Events,
    channels: Channels,
    online: watch::Receiver<bool>,
    stop: watch::Receiver<bool>,
) -> archive::Result<()> {
    worker(
        archive,
        WhatsApp(client),
        events,
        channels,
        online,
        stop,
        Timing::default(),
    )
    .await
}
struct Timing {
    request: Duration,
    retry: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(10),
            retry: Duration::from_secs(30),
        }
    }
}
struct Clear {
    events: Events,
    snapshots: watch::Sender<Snapshot>,
}
impl Drop for Clear {
    fn drop(&mut self) {
        self.events.select(None);
        self.snapshots.send_replace(Snapshot::default());
    }
}

async fn worker(
    archive: Archive,
    transport: impl Transport,
    events: Events,
    channels: Channels,
    mut online: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
    timing: Timing,
) -> archive::Result<()> {
    let Channels {
        mut selected,
        snapshots,
    } = channels;
    let _clear = Clear {
        events: events.clone(),
        snapshots: snapshots.clone(),
    };
    let mut incoming = events.latest.subscribe();
    let mut refresh = true;
    let mut desired: Option<Target> = None;
    let mut subscribed: Option<Target> = None;
    // Record an attempt before awaiting: cancellation/timeout may occur after
    // the server accepted it. Clean it up before subscribing to another peer.
    let mut attempted: Option<String> = None;
    let mut retry_at = Instant::now();
    let mut minute = tokio::time::interval(Duration::from_secs(60));
    minute.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        // A selection/connection change can race an incoming-event wake-up.
        // Observe it before marking the watches seen, regardless of which
        // select! branch woke us; otherwise resolution waits until the minute tick.
        refresh |= selected.has_changed().unwrap_or(false) || online.has_changed().unwrap_or(false);
        let connected = *online.borrow_and_update();
        let chat = *selected.borrow_and_update();
        if !connected {
            if subscribed.is_some() || desired.is_some() {
                events.select(None);
            }
            desired = None;
            subscribed = None;
        } else if refresh {
            let next = if chat > 0 {
                archive.presence_target(chat).await?
            } else {
                None
            };
            if desired != next {
                events.select(None);
            }
            desired = next;
        }
        refresh = false;
        let update = incoming.borrow_and_update().clone();
        let next = if connected
            && subscribed == desired
            && desired.as_ref().is_some_and(|t| t.chat == update.chat)
        {
            update.snapshot(chrono::Local::now())
        } else {
            Snapshot::default()
        };
        snapshots.send_if_modified(|old| {
            if *old == next {
                false
            } else {
                *old = next;
                true
            }
        });
        let ready = Instant::now() >= retry_at;
        if connected && ready {
            let remove = attempted.is_some() && (subscribed.is_none() || subscribed != desired);
            let add = attempted.is_none() && desired.is_some();
            if remove || add {
                let jid = if remove {
                    attempted.as_ref().unwrap().clone()
                } else {
                    let target = desired.as_ref().unwrap();
                    events.select(Some(target.clone()));
                    attempted = Some(target.jid.clone());
                    target.jid.clone()
                };
                let result = tokio::select! {
                    biased;
                    _=stop.changed()=>return Ok(()),
                    result=selected.changed()=>{if result.is_err(){return Ok(());} None},
                    result=online.changed()=>{if result.is_err(){return Ok(());} None},
                    result=timeout(timing.request,async {
                        if remove {transport.unsubscribe(&jid).await} else {transport.subscribe(&jid).await}
                    })=>Some(result.unwrap_or(false)),
                };
                refresh = true;
                match result {
                    Some(true) if remove => {
                        attempted = None;
                        subscribed = None;
                    }
                    Some(true) => subscribed = desired.clone(),
                    Some(false) => {
                        events.select(None);
                        subscribed = None;
                        retry_at = Instant::now() + timing.retry;
                    }
                    None => {
                        events.select(None);
                        subscribed = None;
                    }
                }
                continue;
            }
        }
        tokio::select! {
            biased;
            _=stop.changed()=>return Ok(()),
            result=selected.changed()=>{if result.is_err(){return Ok(());} refresh=true;},
            result=online.changed()=>{if result.is_err(){return Ok(());} refresh=true;},
            _=incoming.changed()=>{},
            _=minute.tick()=>refresh=true,
            _=sleep_until(retry_at),if connected && !ready=>{},
        }
    }
}

#[cfg(test)]
#[path = "protocol_presence_tests.rs"]
mod tests;
