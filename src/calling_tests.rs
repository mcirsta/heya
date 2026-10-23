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
pub(crate) struct Fake {
    pub(crate) delay: AtomicBool,
    pub(crate) resume: tokio::sync::Notify,
    preparing: Arc<AtomicUsize>,
    incoming: watch::Sender<Option<Offer>>,
    pub(crate) opened: Arc<AtomicUsize>,
    pub(crate) ended: Arc<AtomicUsize>,
    declined: AtomicUsize,
    pub(crate) failure: std::sync::Mutex<Option<Error>>,
    ended_remotely: watch::Receiver<bool>,
}
struct Active {
    connected: bool,
    ended: Arc<AtomicUsize>,
    muted: AtomicBool,
    remote: watch::Receiver<bool>,
    _ports: Ports,
    video: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for Active {
    fn drop(&mut self) {
        if let Some(video) = &self.video {
            video.abort();
        }
    }
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
    async fn end(&mut self) {
        if let Some(video) = self.video.take() {
            video.abort();
            let _ = video.await;
        }
        self.ended.fetch_add(1, Ordering::AcqRel);
    }
    async fn camera(&mut self, _: bool) -> Result<(), Error> {
        Ok(())
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
        video: Option<crate::call_video::Ports>,
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
        if let Some(error) = *self.failure.lock().unwrap() {
            return Err(error);
        }
        if self.delay.load(Ordering::Acquire) {
            self.resume.notified().await;
        }
        let video = video.map(|ports| {
            tokio::spawn(async move {
                while let Ok(frame) = ports.capture.recv().await {
                    if ports.playout.send(frame).await.is_err() {
                        break;
                    }
                }
            })
        });
        Ok(Box::new(Active {
            connected: false,
            ended: self.ended.clone(),
            muted: AtomicBool::new(false),
            remote: self.ended_remotely.clone(),
            _ports: ports,
            video,
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

#[tokio::test]
async fn a_queued_answer_or_failure_prevents_a_late_ringback() {
    struct Scripted(Option<Event>);
    #[async_trait::async_trait]
    impl Session for Scripted {
        async fn event(&mut self) -> Event {
            match self.0.take() {
                Some(event) => event,
                None => std::future::pending().await,
            }
        }
        async fn set_muted(&self, _: bool) {}
        fn muted(&self) -> bool {
            false
        }
        async fn end(&mut self) {}
    }
    for event in [
        Event::Accepted,
        Event::Connected,
        Event::Ended,
        Event::Failed(Error::Network),
    ] {
        let mut call = Scripted(Some(event));
        assert!(
            crate::calling::event_or_ring(&mut call, true)
                .await
                .is_some(),
            "a queued backend result must be consumed before any sound starts"
        );
    }
    let mut call = Scripted(None);
    assert!(
        crate::calling::event_or_ring(&mut call, true)
            .await
            .is_none()
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(10),
            crate::calling::event_or_ring(&mut call, false)
        )
        .await
        .is_err(),
        "after acceptance there is no ready branch to restart ringback or spin at idle"
    );
}
pub(crate) struct Harness {
    _temp: tempfile::TempDir,
    pub(crate) archive: crate::archive::Archive,
    pub(crate) chat: i32,
    pub(crate) network: Arc<Fake>,
    _archive_source: watch::Sender<Option<crate::archive::Archive>>,
    _preferences: watch::Sender<Preferences>,
    sources: watch::Sender<Sources>,
    pub(crate) commands: mpsc::Sender<Command>,
    pub(crate) states: watch::Receiver<Status>,
    pub(crate) frames: watch::Receiver<crate::call_video::Frames>,
    remote: watch::Sender<bool>,
    pub(crate) stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
impl Harness {
    pub(crate) async fn new(input: Input) -> Self {
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
            resume: tokio::sync::Notify::new(),
            preparing: Arc::default(),
            incoming: watch::channel(None).0,
            opened: Arc::default(),
            ended: Arc::default(),
            declined: AtomicUsize::new(0),
            failure: Default::default(),
            ended_remotely,
        });
        let (sources, source_rx) = watch::channel(Some(network.clone() as Arc<dyn Network>));
        let (commands, requests) = mpsc::channel(8);
        let (updates, states) = watch::channel(Status::default());
        let (preferences, prefs) = watch::channel(Preferences::default());
        let (archive_source, archives) = watch::channel(Some(archive.clone()));
        let (stop, stopped) = watch::channel(false);
        let (video, frames) = watch::channel(crate::call_video::Frames::default());
        let task = tokio::spawn(crate::calling::run(
            archives,
            source_rx,
            requests,
            crate::calling::Updates {
                status: updates,
                video,
            },
            prefs,
            input,
            stopped,
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
            frames,
            remote,
            stop,
            task,
        }
    }
    pub(crate) async fn finish(self) {
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
        video: false,
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
        video: false,
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
    let failure = state(&mut h.states, |s| {
        s.error == Some(Error::Audio(AudioError::NoInput))
    })
    .await;
    assert_eq!(failure.phase, 0, "a failed check is not a started call");
    assert_eq!(h.network.opened.load(Ordering::Acquire), 0);
    let target = h.archive.call_target(h.chat).await.unwrap();
    h.network.incoming.send_replace(Some(Offer {
        video: false,
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
async fn outgoing_setup_errors_return_to_idle_without_a_call_ended_state() {
    for error in [Error::Network, Error::Unavailable, Error::Offline] {
        let mut h = Harness::new(Input::Synthetic).await;
        state(&mut h.states, |s| s.available).await;
        let chat = match error {
            Error::Network => {
                *h.network.failure.lock().unwrap() = Some(error);
                h.chat
            }
            Error::Offline => {
                h.sources.send_replace(None);
                state(&mut h.states, |s| !s.available).await;
                h.chat
            }
            _ => -1,
        };
        h.commands.send(Command::Start(chat)).await.unwrap();
        let failed = state(&mut h.states, |s| s.error == Some(error)).await;
        assert_eq!(failed.phase, 0);
        assert_eq!(h.network.ended.load(Ordering::Acquire), 0);
        assert_eq!(h.network.preparing.load(Ordering::Acquire), 0);
        assert_eq!(
            h.network.opened.load(Ordering::Acquire),
            usize::from(error == Error::Network)
        );
        h.finish().await;
    }
}
#[tokio::test]
async fn blocked_audio_setup_is_cancellable_and_times_out_before_any_signaling() {
    let mut h = Harness::new(Input::Stalled).await;
    state(&mut h.states, |s| s.available).await;
    h.commands.send(Command::Start(h.chat)).await.unwrap();
    state(&mut h.states, |s| s.phase == 2).await;
    h.stop.send_replace(true);
    let cancelled = state(&mut h.states, |s| s.phase == 0).await;
    assert_eq!(cancelled.error, None);
    h.stop.send_replace(false);
    h.commands.send(Command::Start(h.chat)).await.unwrap();
    let failed = tokio::time::timeout(
        Duration::from_secs(11),
        h.states
            .wait_for(|s| s.phase == 0 && s.error == Some(Error::Audio(AudioError::Unresponsive))),
    )
    .await
    .unwrap()
    .unwrap()
    .clone();
    assert_eq!(failed.seconds, 0);
    assert_eq!(h.network.opened.load(Ordering::Acquire), 0);
    assert_eq!(h.network.preparing.load(Ordering::Acquire), 0);
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
            state(&mut h.states, |s| s.phase == 0).await;
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
        video: false,
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
        video: false,
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
async fn disconnect_and_close_stop_an_incoming_ringtone_without_opening_the_microphone() {
    for disconnect in [true, false] {
        let mut h = Harness::new(Input::Synthetic).await;
        h.network.incoming.send_replace(Some(Offer {
            video: false,
            id: "synthetic-ringing-close".into(),
            target: h.archive.call_target(h.chat).await.unwrap(),
            expires: tokio::time::Instant::now() + Duration::from_secs(90),
        }));
        state(&mut h.states, |s| s.phase == 1).await;
        // Give the real finite silent player time to begin before shutting down.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(h.network.opened.load(Ordering::Acquire), 0);
        if disconnect {
            h.sources.send_replace(None);
            state(&mut h.states, |s| s.phase == 6 && !s.available).await;
        }
        h.finish().await;
    }
}
#[tokio::test]
async fn stale_incoming_controls_cannot_answer_or_decline_a_new_offer() {
    let mut h = Harness::new(Input::Synthetic).await;
    let target = h.archive.call_target(h.chat).await.unwrap();
    h.network.incoming.send_replace(Some(Offer {
        video: false,
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

#[tokio::test]
async fn direct_video_calls_receive_send_pause_camera_and_close_in_one_flow() {
    for incoming in [false, true] {
        let mut h = Harness::new(Input::Synthetic).await;
        if incoming {
            h.network.incoming.send_replace(Some(Offer {
                id: "synthetic-video-call".into(),
                target: h.archive.call_target(h.chat).await.unwrap(),
                expires: tokio::time::Instant::now() + Duration::from_secs(90),
                video: true,
            }));
            state(&mut h.states, |s| s.phase == 1 && s.video).await;
            assert!(
                !h.frames.borrow().camera_on,
                "ringing must not open a camera"
            );
            assert_eq!(h.network.opened.load(Ordering::Acquire), 0);
            h.commands
                .send(Command::Answer("synthetic-video-call".into()))
                .await
                .unwrap();
        } else {
            h.commands.send(Command::StartVideo(h.chat)).await.unwrap();
        }
        state(&mut h.states, |s| s.phase == 5 && s.video).await;
        tokio::time::timeout(
            Duration::from_secs(3),
            h.frames
                .wait_for(|f| f.local.is_some() && f.remote.is_some() && f.camera_on),
        )
        .await
        .unwrap()
        .unwrap();
        h.commands.send(Command::Camera).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), h.frames.wait_for(|f| !f.camera_on))
            .await
            .unwrap()
            .unwrap();
        state(&mut h.states, |s| s.phase == 5 && !s.camera_busy).await;
        assert!(h.frames.borrow().local.is_none());
        h.commands.send(Command::Camera).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), h.frames.wait_for(|f| f.camera_on))
            .await
            .unwrap()
            .unwrap();
        h.stop.send_replace(true);
        state(&mut h.states, |s| s.phase == 6).await;
        assert!(h.frames.borrow().local.is_none() && h.frames.borrow().remote.is_none());
        assert_eq!(h.network.opened.load(Ordering::Acquire), 1);
        assert_eq!(h.network.ended.load(Ordering::Acquire), 1);
        h.finish().await;
    }
}

#[tokio::test]
async fn closing_during_video_setup_or_an_active_video_call_releases_both_helpers() {
    for input in [Input::Synthetic, Input::Stalled] {
        let mut h = Harness::new(input).await;
        let frames = h.frames.clone();
        let network = h.network.clone();
        h.commands.send(Command::StartVideo(h.chat)).await.unwrap();
        let phase = if matches!(input, Input::Synthetic) {
            5
        } else {
            2
        };
        state(&mut h.states, |s| s.phase == phase && s.video).await;
        h.finish().await;
        assert!(!frames.borrow().camera_on);
        assert!(frames.borrow().local.is_none() && frames.borrow().remote.is_none());
        assert_eq!(
            network.opened.load(Ordering::Acquire),
            usize::from(matches!(input, Input::Synthetic))
        );
    }
}
