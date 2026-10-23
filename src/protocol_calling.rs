//! Live calling through the pinned SDK's public facade. No media protocol copy.
use crate::{
    archive::Archive,
    call_audio::Ports,
    calling::{Error, Event, Network, Offer, Session, Target},
};
use futures_util::{Stream, StreamExt};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
use whatsapp_rust::{
    CallError, Client,
    prelude::Event as SdkEvent,
    voip::{CallEvent, CallEvents, CallHandle},
    wacore::types::call::{CallAction, IncomingCall},
    wacore::voip_control::MediaCloseReason,
    wacore_binary::Jid,
};

#[derive(Clone, Default)]
struct Signal {
    id: String,
    accepted: bool,
    ended: bool,
    declined: bool,
}
#[derive(Default)]
struct Pending {
    offer: Option<IncomingCall>,
    active: Option<String>,
    signal: Option<mpsc::Sender<Signal>>,
}
pub struct Calling {
    client: Arc<Client>,
    archive: Archive,
    pending: Arc<Mutex<Pending>>,
    offered: watch::Sender<Option<Offer>>,
}
impl Calling {
    pub fn new(client: Arc<Client>, archive: Archive) -> Self {
        Self {
            client,
            archive,
            pending: Arc::new(Mutex::new(Pending::default())),
            offered: watch::channel(None).0,
        }
    }
    pub async fn observe(&self, event: &SdkEvent) {
        let ended = match event {
            SdkEvent::MissedCall(c) => Some(c.call_id.as_str()),
            SdkEvent::CallEndedElsewhere(c) => Some(c.call_id.as_str()),
            _ => None,
        };
        if let Some(id) = ended {
            self.signal(Signal {
                id: id.into(),
                ended: true,
                ..Default::default()
            });
            self.clear_offer(id);
            return;
        }
        let SdkEvent::IncomingCall(incoming) = event else {
            return;
        };
        let id = incoming.action.call_id();
        if id.is_empty() || id.len() > 128 {
            return;
        }
        match &incoming.action {
            CallAction::Offer {
                is_video: false,
                group_jid: None,
                ..
            } if !incoming.offline => {
                {
                    let pending = self.pending.lock().unwrap();
                    if pending.active.is_some()
                        || pending
                            .offer
                            .as_ref()
                            .is_some_and(|o| o.action.call_id() != id)
                    {
                        return;
                    }
                }
                let Ok(target) = self
                    .archive
                    .incoming_call_target(incoming.from.to_non_ad().to_string())
                    .await
                else {
                    return;
                };
                let mut pending = self.pending.lock().unwrap();
                if pending.active.is_some() {
                    return;
                }
                if pending
                    .offer
                    .as_ref()
                    .is_some_and(|o| o.action.call_id() != id)
                {
                    return;
                }
                pending.offer = Some(incoming.as_ref().clone());
                let expires = self
                    .offered
                    .borrow()
                    .as_ref()
                    .filter(|o| o.id == id)
                    .map_or_else(
                        || tokio::time::Instant::now() + std::time::Duration::from_secs(90),
                        |o| o.expires,
                    );
                let offer = Some(Offer {
                    id: id.into(),
                    target,
                    expires,
                });
                self.offered.send_if_modified(|old| {
                    if old != &offer {
                        *old = offer;
                        true
                    } else {
                        false
                    }
                });
            }
            CallAction::Accept { .. } => {
                self.signal(Signal {
                    id: id.into(),
                    accepted: true,
                    ..Default::default()
                });
            }
            CallAction::Reject { reason: None, .. } | CallAction::Terminate { .. } => {
                let declined = matches!(&incoming.action, CallAction::Reject { .. });
                self.signal(Signal {
                    id: id.into(),
                    ended: true,
                    declined,
                    ..Default::default()
                });
                self.clear_offer(id);
            }
            _ => {}
        }
    }
    fn signal(&self, signal: Signal) {
        let mut pending = self.pending.lock().unwrap();
        if pending.active.as_ref().is_some_and(|id| id != &signal.id) {
            return;
        }
        // Keep setup transitions in order; coalescing Accept with a later,
        // unrelated control event could otherwise leave an audible call ringing.
        if let Some(sender) = &pending.signal
            && sender.try_send(signal).is_err()
        {
            pending.signal = None;
        }
    }
    pub fn disconnected(&self) {
        let mut pending = self.pending.lock().unwrap();
        pending.offer = None;
        pending.signal = None;
        self.offered.send_replace(None);
    }
    fn clear_offer(&self, id: &str) {
        let mut pending = self.pending.lock().unwrap();
        if pending
            .offer
            .as_ref()
            .is_some_and(|o| o.action.call_id() == id)
        {
            pending.offer = None;
            self.offered.send_replace(None);
        }
    }
}
#[async_trait::async_trait]
impl Network for Calling {
    fn offers(&self) -> watch::Receiver<Option<Offer>> {
        self.offered.subscribe()
    }
    async fn open(
        &self,
        target: &Target,
        offer: Option<&str>,
        ports: Ports,
    ) -> Result<Box<dyn Session>, Error> {
        // Recheck lock/read-only/own-account state immediately before signaling.
        let current = self
            .archive
            .call_target(target.chat)
            .await
            .map_err(|_| Error::Unavailable)?;
        // A PN/LID merge may have canonicalized this same captured chat while
        // microphone permission was pending. Use its current stable identity.
        let peer: Jid = current.peer.parse().map_err(|_| Error::Unavailable)?;
        if self
            .client
            .lid()
            .is_some_and(|own| own.to_non_ad() == peer.to_non_ad())
        {
            return Err(Error::Unavailable);
        }
        let (sender, signal) = mpsc::channel(16);
        self.pending.lock().unwrap().signal = Some(sender);
        struct Setup(Arc<Mutex<Pending>>, bool);
        impl Drop for Setup {
            fn drop(&mut self) {
                if self.1 {
                    self.0.lock().unwrap().signal = None;
                }
            }
        }
        let mut setup = Setup(self.pending.clone(), true);
        let voip = self.client.voip();
        let incoming = offer.and_then(|id| {
            self.pending
                .lock()
                .unwrap()
                .offer
                .clone()
                .filter(|o| o.action.call_id() == id)
        });
        eprintln!("Heya call signaling: starting");
        let handle = if let Some(id) = offer {
            let incoming = incoming.ok_or(Error::Unavailable)?;
            if incoming.action.call_id() != id {
                return Err(Error::Unavailable);
            }
            voip.accept(&incoming)
                .audio(ports.microphone, ports.speaker)
                .start()
                .await
        } else {
            voip.call(&peer)
                .audio(ports.microphone, ports.speaker)
                .start()
                .await
        };
        let handle = match handle {
            Ok(h) => h,
            Err(error) => {
                // SDK error strings may contain account/network data. Log only
                // the fixed category, never the raw error or call identifiers.
                eprintln!("Heya call signaling failed: {}", setup_failure(&error));
                if let Some(id) = offer {
                    self.clear_offer(id);
                }
                return Err(Error::Network);
            }
        };
        eprintln!(
            "Heya call signaling: {} sent",
            if offer.is_some() { "answer" } else { "offer" }
        );
        let Some(events) = handle.take_events() else {
            let _ = handle.terminate().await;
            return Err(Error::Network);
        };
        self.pending.lock().unwrap().active = Some(handle.call_id().into());
        setup.1 = false;
        Ok(Box::new(Active {
            handle,
            events,
            signal,
            pending: self.pending.clone(),
            offered: self.offered.clone(),
            accepted: offer.is_some(),
            relay: false,
            reported: false,
        }))
    }
    async fn decline(&self, id: &str) {
        let offer = self
            .pending
            .lock()
            .unwrap()
            .offer
            .clone()
            .filter(|o| o.action.call_id() == id);
        if let Some(offer) = offer {
            let _ = self.client.voip().reject(&offer).await;
        }
        self.clear_offer(id);
    }
}
fn setup_failure(error: &CallError) -> &'static str {
    match error {
        CallError::NoDevices => "no recipient devices",
        CallError::MissingDeviceIdentity => "missing device identity",
        CallError::Setup(_) => "device or session setup",
        CallError::Send(_) => "signaling send",
        CallError::Connect(_) => "relay connection",
        CallError::Decrypt(_) => "offer decryption",
        CallError::Media(_) | CallError::MissingAudio | CallError::AudioFormatNotOffered(_) => {
            "media configuration"
        }
        CallError::CallEndedDuringSetup => "ended during setup",
        CallError::ResponseTimeout => "service timeout",
        CallError::Response(_) => "service response",
        _ => "unsupported call setup",
    }
}
struct Active {
    handle: CallHandle,
    events: CallEvents,
    signal: mpsc::Receiver<Signal>,
    pending: Arc<Mutex<Pending>>,
    offered: watch::Sender<Option<Offer>>,
    accepted: bool,
    relay: bool,
    reported: bool,
}
// Sticky completion and a queued failure can become ready together. Preserve the
// SDK's terminal event before falling back to its completion notification.
async fn receive_media(
    events: &mut (impl Stream<Item = CallEvent> + Unpin),
    ended: impl std::future::Future<Output = ()>,
) -> Option<CallEvent> {
    tokio::select! {
        biased;
        event = events.next() => event,
        _ = ended => None,
    }
}
fn media_failure(event: &CallEvent) -> Option<&'static str> {
    match event {
        CallEvent::RelayAllocateFailed(_) => Some("relay rejected allocation"),
        CallEvent::RelayAllocateTimedOut => Some("relay allocation timeout"),
        CallEvent::RelayReconnectTimedOut => Some("relay reconnect timeout"),
        CallEvent::MediaSetupFailed(_) | CallEvent::Closed(MediaCloseReason::SetupFailed(_)) => {
            Some("media setup")
        }
        CallEvent::Closed(MediaCloseReason::SendFailed(_)) => Some("media send"),
        CallEvent::Closed(MediaCloseReason::RelayDisconnected) => Some("relay disconnected"),
        CallEvent::AudioFormatMismatch { .. } | CallEvent::AudioCodecSourceIsFixed { .. } => {
            Some("audio format")
        }
        _ => None,
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        let stats = self.handle.media_stats();
        eprintln!(
            "Heya call media: received={} decoded={} opus_decoded={} auth_failed={} missing_decoder={} sink_dropped={}",
            stats.rtp_received,
            stats.audio_frames_decoded,
            stats.foreign_frames_decoded,
            stats
                .srtp_unprotect_failed
                .saturating_add(stats.sframe_decrypt_failed),
            stats.audio_frames_without_decoder,
            stats.audio_sink_dropped
        );
        let id = self.handle.call_id();
        let mut pending = self.pending.lock().unwrap();
        if pending.active.as_deref() == Some(id) {
            pending.active = None;
            pending.signal = None;
        }
        if pending
            .offer
            .as_ref()
            .is_some_and(|o| o.action.call_id() == id)
        {
            pending.offer = None;
            self.offered.send_replace(None);
        }
    }
}
#[async_trait::async_trait]
impl Session for Active {
    async fn event(&mut self) -> Event {
        loop {
            if self.accepted && self.relay && !self.reported {
                self.reported = true;
                eprintln!("Heya call signaling: connected");
                return Event::Connected;
            }
            tokio::select! {
                biased;
                signal=self.signal.recv()=>match signal {
                    Some(signal) if signal.id==self.handle.call_id()=>{
                        if signal.declined{return Event::Failed(Error::Declined);}
                        if signal.ended{return Event::Ended;}
                        if signal.accepted&&!self.accepted {eprintln!("Heya call signaling: accepted");self.accepted=true;if !self.relay{return Event::Accepted;}}
                    },
                    Some(_)=>{},None=>return Event::Failed(Error::Offline),
                },
                event=receive_media(&mut self.events, self.handle.wait_ended())=>match event {
                    Some(event) if media_failure(&event).is_some()=>{
                        eprintln!("Heya call media failed: {}", media_failure(&event).unwrap());
                        return Event::Failed(Error::Network);
                    },
                    Some(CallEvent::RelayAllocated)=>{self.relay=true;eprintln!("Heya call signaling: relay ready");},
                    Some(CallEvent::Closed(_))|None=>return Event::Ended,
                    Some(CallEvent::AudioSilent{dominant_reason,..}) if self.reported=>eprintln!("Heya call media: silent ({dominant_reason:?})"),
                    Some(CallEvent::AudioReceptionStalled{..}) if self.reported=>eprintln!("Heya call media: reception stalled"),
                    _=>{},
                }
            }
        }
    }
    async fn set_muted(&self, muted: bool) {
        let _ = self.handle.set_muted(muted).await;
    }
    fn muted(&self) -> bool {
        self.handle.is_muted()
    }
    async fn end(&self) {
        let _ = self.handle.terminate().await;
        self.handle.wait_ended().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatsapp_rust::{
        bot::Bot,
        store::{SqliteDatabase, SqliteDatabaseConfig},
        wacore::types::call::{CallEndedElsewhere, ElsewhereOutcome},
    };

    #[tokio::test]
    async fn queued_media_failure_precedes_sticky_completion() {
        let mut events = futures_util::stream::iter([CallEvent::MediaSetupFailed(
            "Synthetic relay refusal".into(),
        )]);
        assert!(
            matches!(
                receive_media(&mut events, std::future::ready(())).await,
                Some(CallEvent::MediaSetupFailed(_))
            ),
            "completion must not hide the SDK's already queued setup failure"
        );
        assert!(
            receive_media(&mut events, std::future::ready(()))
                .await
                .is_none()
        );
        let mut empty = futures_util::stream::pending();
        assert!(
            receive_media(&mut empty, std::future::ready(()))
                .await
                .is_none(),
            "completion must still wake a call with no diagnostic events"
        );
    }

    #[test]
    fn failure_categories_preserve_transport_errors_without_logging_sdk_text() {
        assert_eq!(
            setup_failure(&CallError::Setup("Private SDK details".into())),
            "device or session setup"
        );
        for event in [
            CallEvent::MediaSetupFailed("Private SDK details".into()),
            CallEvent::Closed(MediaCloseReason::SetupFailed("Private SDK details".into())),
            CallEvent::Closed(MediaCloseReason::SendFailed("Private SDK details".into())),
            CallEvent::Closed(MediaCloseReason::RelayDisconnected),
            CallEvent::RelayAllocateFailed(500),
            CallEvent::RelayAllocateTimedOut,
            CallEvent::RelayReconnectTimedOut,
        ] {
            let category =
                media_failure(&event).expect("transport failure is not an ordinary hangup");
            assert!(!category.contains("Private"));
        }
        assert!(media_failure(&CallEvent::Closed(MediaCloseReason::Local)).is_none());
        assert!(media_failure(&CallEvent::RelayAllocated).is_none());
    }

    async fn fixture() -> (tempfile::TempDir, Calling) {
        let (temp, archive) = crate::archive::tests::fixture();
        let database = SqliteDatabase::open(
            temp.path().join("synthetic-sdk.sqlite").to_str().unwrap(),
            SqliteDatabaseConfig::default(),
        )
        .await
        .unwrap();
        // Constructing a bot does not spawn its transport or contact an account.
        let bot = Bot::builder()
            .with_backend(database.store(1))
            .build()
            .await
            .unwrap();
        (temp, Calling::new(bot.client(), archive))
    }
    fn event(action: CallAction) -> SdkEvent {
        SdkEvent::IncomingCall(Box::new(
            IncomingCall::builder()
                .from("15550001111@s.whatsapp.net".parse().unwrap())
                .stanza_id("synthetic-stanza".into())
                .offline(false)
                .action(action)
                .build(),
        ))
    }
    fn offer(id: &str, video: bool) -> SdkEvent {
        event(CallAction::Offer {
            call_id: id.into(),
            call_creator: "15550001111@s.whatsapp.net".parse().unwrap(),
            caller_pn: None,
            caller_country_code: None,
            device_class: None,
            joinable: false,
            is_video: video,
            audio: Vec::new(),
            group_jid: None,
        })
    }
    #[tokio::test]
    async fn only_current_audio_offers_ring_and_other_device_completion_clears_them() {
        let (_temp, source) = fixture().await;
        let mut offers = source.offers();
        source.observe(&offer("video", true)).await;
        let mut offline = offer("offline", false);
        if let SdkEvent::IncomingCall(c) = &mut offline {
            c.offline = true;
        }
        source.observe(&offline).await;
        let mut group = offer("group", false);
        if let SdkEvent::IncomingCall(c) = &mut group
            && let CallAction::Offer { group_jid, .. } = &mut c.action
        {
            *group_jid = Some("123456@g.us".parse().unwrap());
        }
        source.observe(&group).await;
        assert!(offers.borrow().is_none());

        source.observe(&offer("audio", false)).await;
        let first = offers.borrow_and_update().clone().unwrap();
        source.observe(&offer("audio", false)).await;
        assert_eq!(offers.borrow().as_ref().unwrap().expires, first.expires);
        assert!(
            !offers.has_changed().unwrap(),
            "retransmission must not refocus the call UI"
        );
        source.observe(&offer("another", false)).await;
        assert_eq!(offers.borrow().as_ref().unwrap().id, "audio");
        source
            .observe(&SdkEvent::CallEndedElsewhere(CallEndedElsewhere::new(
                "15550001111@s.whatsapp.net".parse().unwrap(),
                "audio".into(),
                chrono::Utc::now(),
                ElsewhereOutcome::Accepted,
            )))
            .await;
        assert!(offers.borrow().is_none());
        source.observe(&offer("fresh", false)).await;
        assert_eq!(offers.borrow().as_ref().unwrap().id, "fresh");
        source.disconnected();
        assert!(offers.borrow().is_none());
    }
    #[tokio::test]
    async fn setup_signals_preserve_accept_and_ignore_device_busy_rejections() {
        let (_temp, source) = fixture().await;
        let (tx, mut rx) = mpsc::channel(16);
        source.pending.lock().unwrap().signal = Some(tx);
        let creator: Jid = "15550001111@s.whatsapp.net".parse().unwrap();
        source
            .observe(&event(CallAction::Reject {
                call_id: "audio".into(),
                call_creator: creator.clone(),
                reason: Some("busy".into()),
            }))
            .await;
        assert!(
            rx.try_recv().is_err(),
            "a busy companion does not decline the callee's other devices"
        );
        source
            .observe(&event(CallAction::Accept {
                call_id: "audio".into(),
                call_creator: creator.clone(),
                audio: Vec::new(),
            }))
            .await;
        source
            .observe(&event(CallAction::Terminate {
                call_id: "unrelated".into(),
                call_creator: creator.clone(),
                reason: None,
                duration: None,
                audio_duration: None,
            }))
            .await;
        assert!(
            rx.try_recv().unwrap().accepted,
            "later signaling must not overwrite acceptance"
        );
        assert_eq!(rx.try_recv().unwrap().id, "unrelated");
        source.pending.lock().unwrap().active = Some("audio".into());
        source
            .observe(&event(CallAction::Reject {
                call_id: "unrelated".into(),
                call_creator: creator.clone(),
                reason: None,
            }))
            .await;
        assert!(rx.try_recv().is_err());
        source
            .observe(&event(CallAction::Reject {
                call_id: "audio".into(),
                call_creator: creator,
                reason: None,
            }))
            .await;
        let ended = rx.try_recv().unwrap();
        assert!(ended.ended && ended.declined);
        source.disconnected();
        assert!(rx.recv().await.is_none());
    }
}
