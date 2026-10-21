use super::*;
use crate::{
    archive::{
        Batch,
        tests::{fixture, record},
    },
    audio_player::{Desired, Output},
};
use whatsapp_rust::{
    prelude::wa,
    waproto::{buffa::MessageField, codec},
};

#[path = "audio_lifecycle_tests.rs"]
mod lifecycle;

fn envelope() -> wa::Message {
    let mut clip = wa::message::AudioMessage::default();
    clip.seconds = Some(4);
    clip.ptt = Some(true);
    clip.waveform = Some(vec![255; 100]);
    clip.media_key = Some(vec![1; 32]);
    clip.file_sha256 = Some(vec![2; 32]);
    clip.file_enc_sha256 = Some(vec![3; 32]);
    let mut msg = wa::Message::default();
    msg.audio_message = MessageField::some(clip);
    msg
}
async fn inserted(db: &Archive) -> i32 {
    let mut row = record("synthetic-audio");
    let msg = envelope();
    row.content = crate::protocol_content::extract(&msg).1;
    row.raw = codec::message_to_vec(&msg);
    db.ingest(Batch {
        messages: vec![row],
        ..Default::default()
    })
    .await
    .unwrap();
    db.blocking(|c| {
        Ok(c.query_row(
            "SELECT id FROM messages WHERE wire_id='synthetic-audio'",
            [],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}
async fn until(rx: &mut watch::Receiver<Status>, test: impl Fn(&Status) -> bool) -> Status {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let s = rx.borrow_and_update().clone();
            if test(&s) {
                return s;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .expect("audio state deadline")
}

#[test]
fn metadata_is_bounded_and_view_once_stays_unopened() {
    let mut msg = envelope();
    let c = crate::protocol_content::extract(&msg).1;
    let a = c.audio.unwrap();
    assert!(a.voice);
    assert_eq!(a.seconds, Some(4));
    assert_eq!(a.waveform, vec![100; 64]);
    let mut clip = msg.audio_message.as_option().unwrap().clone();
    clip.view_once = Some(true);
    msg.audio_message = MessageField::some(clip);
    assert!(crate::protocol_content::extract(&msg).1.audio.is_none());
}

#[cfg(any(
    target_os = "macos",
    all(feature = "media-gstreamer", target_os = "linux")
))]
#[tokio::test]
async fn encrypted_cache_guards_mutations_limits_and_offline_loading() {
    let (_temp, db) = fixture();
    let id = inserted(&db).await;
    let request = db.audio_request(id).await.unwrap().unwrap();
    assert!(matches!(
        load(Some(db.clone()), None, id, false).await,
        Err(Error::Offline)
    ));
    let bytes = crate::fixtures::audio_bytes();
    assert!(db.save_audio(&request, bytes.clone()).await.unwrap());
    assert_eq!(
        &*load(Some(db.clone()), None, id, false).await.unwrap().1,
        &bytes
    );
    assert_eq!(
        db.save_audio(&request, vec![0; MAX_BYTES + 1]).await,
        Err(crate::archive::Error::Limit)
    );
    db.blocking(|c| {
        c.execute("UPDATE messages SET deleted=1,raw=X'',content=''", [])?;
        Ok(())
    })
    .unwrap();
    assert!(db.cached_audio(&request).await.unwrap().is_none());
    assert!(!db.save_audio(&request, bytes).await.unwrap());
    assert!(matches!(
        load(Some(db), None, id, false).await,
        Err(Error::Unavailable)
    ));
}

// Exercise the real installed Ogg/Opus encoder and decoder, without any desktop
// audio device. No real account media or encoded fixture files are involved.
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
fn opus() -> Vec<u8> {
    use gstreamer::{self as gst, prelude::*};
    gst::init().unwrap();
    let pipeline=gst::parse::launch("audiotestsrc wave=silence num-buffers=200 samplesperbuffer=960 ! audio/x-raw,rate=48000,channels=1 ! opusenc ! oggmux ! appsink name=output sync=false").unwrap().downcast::<gst::Pipeline>().unwrap();
    let sink = pipeline
        .by_name("output")
        .unwrap()
        .downcast::<gstreamer_app::AppSink>()
        .unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();
    let mut bytes = Vec::new();
    while let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_seconds(5)) {
        bytes.extend_from_slice(sample.buffer().unwrap().map_readable().unwrap().as_slice());
    }
    pipeline.set_state(gst::State::Null).unwrap();
    assert!(bytes.starts_with(b"OggS"));
    bytes
}

#[cfg(target_os = "macos")]
fn opus() -> Vec<u8> {
    crate::macos_audio::test_opus(48000, 1, 4)
}

#[cfg(any(
    target_os = "macos",
    all(feature = "media-gstreamer", target_os = "linux")
))]
#[tokio::test(flavor = "multi_thread")]
async fn opus_pause_seek_speed_eos_replace_and_bad_media() {
    let bytes = tokio::task::spawn_blocking(opus).await.unwrap();
    let (updates, mut status) = watch::channel(Status::default());
    let mut desired = Desired {
        generation: 1,
        id: 1,
        bytes: Some(bytes.into()),
        playing: true,
        speed: 1.,
        seek: None,
    };
    let mut worker = Worker::start(desired.clone(), updates.clone(), Output::Silent).unwrap();
    let playing = until(&mut status, |s| s.phase == 2 && s.position > 0.1).await;
    assert!((playing.duration - 4.).abs() < 0.1);
    desired.playing = false;
    worker.update(&desired);
    let paused = until(&mut status, |s| s.phase == 3).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!((status.borrow().position - paused.position).abs() < 0.1);
    desired.seek = Some((1, 0.5));
    desired.speed = 1.5;
    worker.update(&desired);
    until(&mut status, |s| {
        s.phase == 3 && s.position >= 1.9 && s.speed == 1.5
    })
    .await;
    desired.playing = true;
    desired.speed = 2.;
    worker.update(&desired);
    until(&mut status, |s| s.phase == 2 && s.speed == 2.).await;
    until(&mut status, |s| s.phase == 0 && s.id == 1).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), worker.finished())
            .await
            .unwrap()
            .unwrap()
            .phase,
        0
    );
    drop(worker);
    desired.generation += 1;
    desired.id = 2;
    desired.bytes = Some(crate::fixtures::audio_bytes().into());
    desired.seek = None;
    desired.speed = 1.;
    let mut worker = Worker::start(desired.clone(), updates.clone(), Output::Silent).unwrap();
    until(&mut status, |s| s.phase == 2 && s.id == 2).await;
    worker.stop();
    tokio::time::timeout(Duration::from_secs(2), worker.finished())
        .await
        .unwrap()
        .unwrap();
    drop(worker);
    desired.generation += 1;
    desired.id = 3;
    desired.bytes = Some(Vec::from(b"invalid synthetic clip").into());
    let mut worker = Worker::start(desired, updates, Output::Silent).unwrap();
    until(&mut status, |s| s.phase == 4 && s.id == 3).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), worker.finished())
            .await
            .unwrap()
            .unwrap()
            .phase,
        4
    );
}

struct Delayed(tokio::sync::Notify);
#[async_trait::async_trait]
impl Source for Delayed {
    async fn fetch(&self, _: &[u8]) -> Result<Vec<u8>, Error> {
        self.0.notify_one();
        std::future::pending().await
    }
}
#[cfg(any(
    target_os = "macos",
    all(feature = "media-gstreamer", target_os = "linux")
))]
#[tokio::test(flavor = "multi_thread")]
async fn cancelled_download_never_starts_playback_or_survives_shutdown() {
    let (_temp, db) = fixture();
    let id = inserted(&db).await;
    let (archives, arx) = watch::channel(Some(db.clone()));
    let source = Arc::new(Delayed(tokio::sync::Notify::new()));
    let (sources, srx) = watch::channel(Some(source.clone() as Arc<dyn Source>));
    let (commands, rx) = mpsc::channel(16);
    let (updates, mut status) = watch::channel(Status::default());
    let worker = tokio::spawn(run(
        arx,
        srx,
        rx,
        updates,
        false,
        watch::channel(crate::preferences::Preferences::default()).1,
    ));
    commands.send(Command::Toggle(id)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), source.0.notified())
        .await
        .unwrap();
    commands.send(Command::Stop).await.unwrap();
    until(&mut status, |s| s.id == 0).await;
    assert!(
        db.cached_audio(&db.audio_request(id).await.unwrap().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    drop(commands);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    drop((archives, sources));
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_and_close_cancel_pending_audio_and_release_its_source() {
    for close_window in [false, true] {
        let (_temp, db) = fixture();
        let id = inserted(&db).await;
        let (_archives, arx) = watch::channel(Some(db.clone()));
        let source = Arc::new(Delayed(tokio::sync::Notify::new()));
        let weak = Arc::downgrade(&source);
        let (sources, srx) = watch::channel(Some(source.clone() as Arc<dyn Source>));
        let (commands, rx) = mpsc::channel(16);
        let (updates, mut status) = watch::channel(Status::default());
        let worker = tokio::spawn(run(
            arx,
            srx,
            rx,
            updates,
            false,
            watch::channel(crate::preferences::Preferences::default()).1,
        ));
        commands.send(Command::Toggle(id)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), source.0.notified())
            .await
            .unwrap();
        drop(source);
        sources.send_replace(None);
        if !close_window {
            until(&mut status, |s| s.error == Some(Error::Offline)).await;
            assert!(
                weak.upgrade().is_none(),
                "disconnect must release the SDK owner"
            );
        }
        assert!(
            db.cached_audio(&db.audio_request(id).await.unwrap().unwrap())
                .await
                .unwrap()
                .is_none()
        );
        drop(commands);
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        assert!(
            weak.upgrade().is_none(),
            "window close must release the SDK owner"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cached_playback_stops_on_expiry_and_releases_bytes() {
    let (_temp, db) = fixture();
    let id = inserted(&db).await;
    db.save_audio(
        &db.audio_request(id).await.unwrap().unwrap(),
        crate::fixtures::audio_bytes(),
    )
    .await
    .unwrap();
    let (_archives, arx) = watch::channel(Some(db.clone()));
    let (_sources, srx) = watch::channel(None);
    let (commands, rx) = mpsc::channel(16);
    let (updates, mut status) = watch::channel(Status::default());
    let worker = tokio::spawn(run(
        arx,
        srx,
        rx,
        updates,
        false,
        watch::channel(crate::preferences::Preferences::default()).1,
    ));
    commands.send(Command::Toggle(id)).await.unwrap();
    until(&mut status, |s| s.phase == 2).await;
    commands.send(Command::Toggle(id)).await.unwrap();
    until(&mut status, |s| s.phase == 3).await;
    db.blocking(|c| {
        c.execute("UPDATE messages SET expires=1", [])?;
        Ok(())
    })
    .unwrap();
    until(&mut status, |s| s.error == Some(Error::Unavailable)).await;
    drop(commands);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn old_envelopes_migrate_and_cached_audio_reopens_encrypted() {
    use crate::{
        archive::tests::KEY,
        storage::{prepare_at, tests::MemoryVault},
    };
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let db = Archive::open(files.clone(), KEY).unwrap();
    let id = inserted(&db).await;
    db.blocking(|c|{c.execute_batch("DROP TRIGGER discard_changed_document_preview; DROP TABLE document_previews; DROP TABLE outgoing_reads; DROP INDEX messages_action_wire; DROP TABLE reply_drafts; DROP TABLE reactions; DROP TRIGGER discard_changed_audio; DROP TABLE message_audio; UPDATE messages SET content=''; DROP TABLE history_pages; DROP TABLE history_requests; ALTER TABLE messages DROP COLUMN edit_version; ALTER TABLE mutations DROP COLUMN raw; DROP TABLE outgoing_receipts; DROP TABLE chat_settings; DROP INDEX messages_album; ALTER TABLE messages DROP COLUMN album_index; ALTER TABLE messages DROP COLUMN album; PRAGMA user_version=6; INSERT OR REPLACE INTO metadata(key,value) VALUES('content_decoder_revision','2');")?;Ok(())}).unwrap();
    drop(db);
    let db = Archive::open(files.clone(), KEY).unwrap();
    crate::protocol_messages::repair_text(&db).unwrap();
    let request = db.audio_request(id).await.unwrap().unwrap();
    let bytes = crate::fixtures::audio_bytes();
    db.save_audio(&request, bytes.clone()).await.unwrap();
    drop(db);
    let db = Archive::open(files.clone(), KEY).unwrap();
    assert_eq!(db.cached_audio(&request).await.unwrap(), Some(bytes));
    assert!(Archive::open(files, "PRAGMA key='wrong synthetic key';").is_err());
}
