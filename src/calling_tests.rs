//! Bounded fake signaling plus real finite PCM helpers; never calls an account.
use crate::{
    archive::tests::{fixture, record},
    call_audio::{Ports, SAMPLES},
    calling::*,
    preferences::Preferences,
    voice_recorder::{Error as AudioError, Input},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
struct Fake {
    delay: AtomicBool,
    preparing: Arc<AtomicUsize>,
    incoming: watch::Sender<Option<Offer>>,
    opened: Arc<AtomicUsize>,
    ended: Arc<AtomicUsize>,
    declined: AtomicUsize,
    ended_remotely: watch::Receiver<bool>,
}
struct Active {
    connected: bool,
    ended: Arc<AtomicUsize>,
    muted: AtomicBool,
    remote: watch::Receiver<bool>,
    _ports: Ports,
}
#[async_trait::async_trait]
impl Session for Active {
    async fn event(&mut self) -> Event {
        if !self.connected {
            self.connected = true;
            return Event::Connected;
        }
        let _ = self.remote.wait_for(|v| *v).await;
        Event::Ended
    }
    async fn set_muted(&self, value: bool) {
        self.muted.store(value, Ordering::Release);
    }
    fn muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
    }
    async fn end(&self) {
        self.ended.fetch_add(1, Ordering::AcqRel);
    }
}
#[async_trait::async_trait]
impl Network for Fake {
    fn offers(&self) -> watch::Receiver<Option<Offer>> {
        self.incoming.subscribe()
    }
    async fn open(
        &self,
        _: &Target,
        _: Option<&str>,
        ports: Ports,
    ) -> Result<Box<dyn Session>, Error> {
        struct Preparing(Arc<AtomicUsize>);
        impl Drop for Preparing {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }
        self.preparing.fetch_add(1, Ordering::AcqRel);
        let _preparing = Preparing(self.preparing.clone());
        let pcm = ports.microphone.recv().await.map_err(|_| Error::Network)?;
        assert_eq!(pcm.len(), SAMPLES);
        ports.speaker.send(pcm).await.map_err(|_| Error::Network)?;
        self.opened.fetch_add(1, Ordering::AcqRel);
        if self.delay.load(Ordering::Acquire) {
            std::future::pending::<()>().await;
        }
        Ok(Box::new(Active {
            connected: false,
            ended: self.ended.clone(),
            muted: AtomicBool::new(false),
            remote: self.ended_remotely.clone(),
            _ports: ports,
        }))
    }
    async fn decline(&self, _: &str) {
        self.declined.fetch_add(1, Ordering::AcqRel);
        self.incoming.send_replace(None);
    }
}
async fn state(rx: &mut watch::Receiver<Status>, accept: impl Fn(&Status) -> bool) -> Status {
    tokio::time::timeout(Duration::from_secs(6), rx.wait_for(accept))
        .await
        .unwrap()
        .unwrap()
        .clone()
}
struct Harness {
    _temp: tempfile::TempDir,
    archive: crate::archive::Archive,
    chat: i32,
    network: Arc<Fake>,
    _archive_source: watch::Sender<Option<crate::archive::Archive>>,
    _preferences: watch::Sender<Preferences>,
    sources: watch::Sender<Sources>,
    commands: mpsc::Sender<Command>,
    states: watch::Receiver<Status>,
    remote: watch::Sender<bool>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
impl Harness {
    async fn new(input: Input) -> Self {
        let (temp, archive) = fixture();
        archive
            .ingest(crate::archive::Batch {
                messages: vec![record("call-peer")],
                ..Default::default()
            })
            .await
            .unwrap();
        let chat = archive
            .view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats[0]
            .id;
        let (remote, ended_remotely) = watch::channel(false);
        let network = Arc::new(Fake {
            delay: AtomicBool::new(false),
            preparing: Arc::default(),
            incoming: watch::channel(None).0,
            opened: Arc::default(),
            ended: Arc::default(),
            declined: AtomicUsize::new(0),
            ended_remotely,
        });
        let (sources, source_rx) = watch::channel(Some(network.clone() as Arc<dyn Network>));
        let (commands, requests) = mpsc::channel(8);
        let (updates, states) = watch::channel(Status::default());
        let (preferences, prefs) = watch::channel(Preferences::default());
        let (archive_source, archives) = watch::channel(Some(archive.clone()));
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(crate::calling::run(
            archives, source_rx, requests, updates, prefs, input, stopped,
        ));
        Self {
            _temp: temp,
            archive,
            chat,
            network,
            _archive_source: archive_source,
            _preferences: preferences,
            sources,
            commands,
            states,
            remote,
            stop,
            task,
        }
    }
    async fn finish(self) {
        drop(self.commands);
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .unwrap()
            .unwrap();
    }
}
#[tokio::test]
async fn outgoing_audio_can_mute_and_hangup_and_releases_each_helper() {
    let mut h = Harness::new(Input::Synthetic).await;
    for _ in 0..2 {
        h.stop.send_replace(false);
        h.commands.send(Command::Start(h.chat)).await.unwrap();
        state(&mut h.states, |s| s.phase == 5).await;
        h.commands.send(Command::Mute).await.unwrap();
        state(&mut h.states, |s| s.phase == 5 && s.muted).await;
        h.stop.send_replace(true);
        state(&mut h.states, |s| s.phase == 6).await;
        h.commands
            .send(Command::Dismiss(String::new()))
            .await
            .unwrap();
        state(&mut h.states, |s| s.phase == 0).await;
    }
    assert_eq!(h.network.opened.load(Ordering::Acquire), 2);
    assert_eq!(h.network.ended.load(Ordering::Acquire), 2);
    h.finish().await;
}
#[tokio::test]
async fn incoming_audio_can_decline_or_answer_then_end_remotely() {
    let mut h = Harness::new(Input::Synthetic).await;
    let target = h.archive.call_target(h.chat).await.unwrap();
    h.network.incoming.send_replace(Some(Offer {
        id: "synthetic-incoming-1".into(),
        expires: tokio::time::Instant::now() + Duration::from_secs(90),
        target: target.clone(),
    }));
    state(&mut h.states, |s| s.phase == 1).await;
    assert_eq!(
        h.network.opened.load(Ordering::Acquire),
        0,
        "ringing never opens microphone or signaling media"
    );
    h.commands
        .send(Command::Decline("synthetic-incoming-1".into()))
        .await
        .unwrap();
    state(&mut h.states, |s| s.phase == 6).await;
    assert_eq!(h.network.declined.load(Ordering::Acquire), 1);
    h.network.incoming.send_replace(Some(Offer {
        id: "synthetic-incoming-2".into(),
        expires: tokio::time::Instant::now() + Duration::from_secs(90),
        target,
    }));
    state(&mut h.states, |s| s.phase == 1).await;
    h.commands
        .send(Command::Answer("synthetic-incoming-2".into()))
        .await
        .unwrap();
    state(&mut h.states, |s| s.phase == 5).await;
    h.remote.send_replace(true);
    state(&mut h.states, |s| s.phase == 6).await;
    assert_eq!(h.network.ended.load(Ordering::Acquire), 1);
    h.finish().await;
}
#[tokio::test]
async fn missing_microphone_never_signals_a_call_and_an_offer_remains_answerable() {
    let mut h = Harness::new(Input::Unavailable).await;
    h.commands.send(Command::Start(h.chat)).await.unwrap();
    state(&mut h.states, |s| {
        s.error == Some(Error::Audio(AudioError::NoInput))
    })
    .await;
    assert_eq!(h.network.opened.load(Ordering::Acquire), 0);
    let target = h.archive.call_target(h.chat).await.unwrap();
    h.network.incoming.send_replace(Some(Offer {
        id: "synthetic-no-mic".into(),
        expires: tokio::time::Instant::now() + Duration::from_secs(90),
        target,
    }));
    state(&mut h.states, |s| s.phase == 1 && s.error.is_none()).await;
    h.commands
        .send(Command::Answer("synthetic-no-mic".into()))
        .await
        .unwrap();
    state(&mut h.states, |s| {
        s.phase == 1 && s.error == Some(Error::Audio(AudioError::NoInput))
    })
    .await;
    assert!(h.network.incoming.borrow().is_some());
    assert_eq!(h.network.opened.load(Ordering::Acquire), 0);
    h.finish().await;
}
#[tokio::test]
async fn disconnect_and_closing_the_ui_end_an_active_call() {
    for disconnect in [true, false] {
        let mut h = Harness::new(Input::Synthetic).await;
        h.commands.send(Command::Start(h.chat)).await.unwrap();
        state(&mut h.states, |s| s.phase == 5).await;
        if disconnect {
            h.sources.send_replace(None);
            state(&mut h.states, |s| {
                s.phase == 6 && s.error == Some(Error::Offline) && !s.available
            })
            .await;
            assert_eq!(h.network.ended.load(Ordering::Acquire), 1);
            h.finish().await;
        } else {
            let ended = h.network.ended.clone();
            h.finish().await;
            assert_eq!(ended.load(Ordering::Acquire), 1);
        }
    }
}
#[tokio::test]
async fn call_targets_reject_groups_self_readonly_and_locked_chats() {
    let h = Harness::new(Input::Synthetic).await;
    assert!(h.archive.call_target(h.chat).await.is_ok());
    let group = h
        .archive
        .create_chat("15550000001-123@g.us".into())
        .await
        .unwrap();
    assert!(h.archive.call_target(group).await.is_err());
    use crate::archive::{Batch, ChatMeta};
    use crate::chat_management::{Setting, Update};
    h.archive
        .ingest(Batch {
            chats: vec![ChatMeta {
                jid: crate::archive::tests::PN.into(),
                read_only: Some(true),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(h.archive.call_target(h.chat).await.is_err());
    h.archive
        .ingest(Batch {
            chats: vec![ChatMeta {
                jid: crate::archive::tests::PN.into(),
                read_only: Some(false),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    h.archive
        .save_chat_settings(vec![Update {
            jid: crate::archive::tests::PN.into(),
            setting: Setting::Lock,
            value: 1,
            stamp: Some(1),
        }])
        .await
        .unwrap();
    assert!(h.archive.call_target(h.chat).await.is_err());
    h.archive
        .save_chat_settings(vec![Update {
            jid: crate::archive::tests::PN.into(),
            setting: Setting::Lock,
            value: 0,
            stamp: Some(2),
        }])
        .await
        .unwrap();
    assert!(h.archive.call_target(h.chat).await.is_ok());
    let own = crate::archive::tests::PN.to_string();
    h.archive
        .bind_account(own, "Synthetic owner".into())
        .await
        .unwrap();
    assert!(h.archive.call_target(h.chat).await.is_err());
    assert!(h.archive.call_target(-1).await.is_err());
    h.finish().await;
}
#[tokio::test]
async fn cancelling_or_closing_during_call_preparation_releases_audio_and_setup() {
    for closing in [false, true] {
        let mut h = Harness::new(Input::Synthetic).await;
        h.network.delay.store(true, Ordering::Release);
        h.commands.send(Command::Start(h.chat)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while h.network.opened.load(Ordering::Acquire) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let preparing = h.network.preparing.clone();
        if closing {
            h.finish().await;
        } else {
            h.stop.send_replace(true);
            state(&mut h.states, |s| s.phase == 6).await;
            h.finish().await;
        }
        assert_eq!(preparing.load(Ordering::Acquire), 0);
    }
}
#[tokio::test]
async fn expired_ringing_and_remote_cancellation_never_open_audio() {
    let mut h = Harness::new(Input::Synthetic).await;
    let target = h.archive.call_target(h.chat).await.unwrap();
    h.network.incoming.send_replace(Some(Offer {
        id: "expired-offer".into(),
        target: target.clone(),
        expires: tokio::time::Instant::now() - Duration::from_secs(1),
    }));
    state(&mut h.states, |s| {
        s.phase == 6 && s.error == Some(Error::Timeout)
    })
    .await;
    assert_eq!(h.network.opened.load(Ordering::Acquire), 0);
    assert_eq!(h.network.declined.load(Ordering::Acquire), 1);
    h.network.incoming.send_replace(Some(Offer {
        id: "cancelled-offer".into(),
        target,
        expires: tokio::time::Instant::now() + Duration::from_secs(90),
    }));
    state(&mut h.states, |s| s.phase == 1).await;
    h.network.incoming.send_replace(None);
    state(&mut h.states, |s| s.phase == 6 && s.error.is_none()).await;
    assert_eq!(h.network.opened.load(Ordering::Acquire), 0);
    h.finish().await;
}
#[tokio::test]
async fn stale_incoming_controls_cannot_answer_or_decline_a_new_offer() {
    let mut h = Harness::new(Input::Synthetic).await;
    let target = h.archive.call_target(h.chat).await.unwrap();
    h.network.incoming.send_replace(Some(Offer {
        id: "new-offer".into(),
        target,
        expires: tokio::time::Instant::now() + Duration::from_secs(90),
    }));
    state(&mut h.states, |s| s.phase == 1 && s.offer == "new-offer").await;
    h.commands
        .send(Command::Answer("old-offer".into()))
        .await
        .unwrap();
    h.commands
        .send(Command::Decline("old-offer".into()))
        .await
        .unwrap();
    h.commands
        .send(Command::Dismiss("old-offer".into()))
        .await
        .unwrap();
    h.commands
        .send(Command::Answer("new-offer".into()))
        .await
        .unwrap();
    state(&mut h.states, |s| s.phase == 5).await;
    assert_eq!(h.network.opened.load(Ordering::Acquire), 1);
    assert_eq!(h.network.declined.load(Ordering::Acquire), 0);
    h.stop.send_replace(true);
    state(&mut h.states, |s| s.phase == 6).await;
    h.finish().await;
}
