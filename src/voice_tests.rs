use super::*;
use crate::{
    attachments::{self, Uploader},
    voice_recorder::{self as recorder, Desired, Input},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
async fn recording() -> recorder::Clip {
    let (tx, mut progress) = watch::channel(recorder::Progress::default());
    let mut worker = recorder::Worker::start(Input::Synthetic, tx, Default::default()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while progress.borrow().seconds < 0.25 {
            progress.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    worker.set(Desired::Pause);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let paused = progress.borrow().seconds;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!((progress.borrow().seconds - paused).abs() < 0.1);
    worker.set(Desired::Record);
    tokio::time::sleep(Duration::from_millis(150)).await;
    worker.set(Desired::Finish);
    tokio::time::timeout(Duration::from_secs(5), worker.finished())
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn opus_capture_pause_finalize_encrypt_envelope_and_offline_cache() {
    use whatsapp_rust::{
        prelude::MessageExt,
        wacore::download::{DownloadUtils, MediaType},
        waproto::codec,
    };
    let clip = recording().await;
    assert!(clip.bytes.starts_with(b"OggS"));
    assert_eq!(clip.waveform.len(), 64);
    assert!(clip.waveform.iter().any(|v| *v > 0));
    let staged = crate::protocol_uploads::stage_voice(clip.clone(), Control::default())
        .await
        .unwrap();
    let destination = attachments::Destination {
        chat: 1,
        title: "Synthetic".into(),
        reply: Some(crate::message_actions::Source {
            id: 1,
            chat: 1,
            jid: "15550000001@s.whatsapp.net".into(),
            sender: "15550000001@s.whatsapp.net".into(),
            wire: "synthetic-reply".into(),
            outgoing: false,
            body: "Reply".into(),
            raw: crate::protocol_uploads::tests::text("Reply"),
        }),
        owner: "15550000000@s.whatsapp.net".into(),
    };
    let prepared = crate::protocol_uploads::tests::prepared(&staged, String::new(), &destination);
    assert_eq!(prepared.text, "[Audio message]");
    let message = codec::message_decode(&prepared.raw).unwrap();
    let audio = message.audio_message.as_option().unwrap();
    assert!(
        audio
            .streaming_sidecar
            .as_ref()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(audio.ptt, Some(true));
    assert_eq!(audio.mimetype.as_deref(), Some("audio/ogg; codecs=opus"));
    assert_eq!(audio.seconds, Some(clip.seconds));
    assert_eq!(audio.waveform.as_deref(), Some(clip.waveform.as_slice()));
    assert_eq!(
        message.context_info().unwrap().stanza_id.as_deref(),
        Some("synthetic-reply")
    );
    let mut bytes = Vec::new();
    DownloadUtils::decrypt_stream_to_writer_with_hashes(
        crate::protocol_uploads::test_reader(staged.clone()),
        &staged.key,
        MediaType::Audio,
        Some(&staged.encrypted_hash),
        Some(&staged.hash),
        &mut bytes,
    )
    .unwrap();
    assert_eq!(bytes.as_slice(), clip.bytes.as_ref());
    let (_temp, db) = crate::archive::tests::fixture();
    let chat = db
        .create_chat(crate::archive::tests::PN.into())
        .await
        .unwrap();
    let prepared = crate::protocol_uploads::tests::prepared(
        &staged,
        String::new(),
        &attachments::Destination {
            chat,
            title: "Synthetic".into(),
            reply: None,
            owner: String::new(),
        },
    );
    db.queue_media(
        chat,
        staged.id.clone(),
        prepared,
        None,
        Some(clip.bytes.clone()),
    )
    .await
    .unwrap();
    let id = db
        .call(|db| Ok(db.query_row("SELECT id FROM messages LIMIT 1", [], |r| r.get(0))?))
        .await
        .unwrap();
    let request = db.audio_request(id).await.unwrap().unwrap();
    assert_eq!(
        db.cached_audio(&request).await.unwrap().unwrap(),
        clip.bytes.as_ref()
    );
    let (updates, status) = watch::channel(recorder::Progress::default());
    let mut player = crate::media_process::Worker::start(
        crate::media_process::Mode::Preview {
            bytes: clip.bytes.clone(),
            silent: true,
            settings: Default::default(),
        },
        updates,
    )
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), player.finished())
            .await
            .unwrap()
            .unwrap(),
        crate::media_process::Outcome::End
    ));
    assert!(status.borrow().seconds > 0.1);
}
struct Fake {
    fail: AtomicBool,
    last: Mutex<Option<recorder::Clip>>,
}
#[async_trait::async_trait]
impl Uploader for Fake {
    async fn upload(
        &self,
        file: Arc<attachments::Staged>,
        caption: String,
        d: &attachments::Destination,
        control: Control,
        _: watch::Sender<u64>,
    ) -> Result<crate::message_actions::Prepared, attachments::Error> {
        assert_eq!(file.kind, attachments::Kind::Voice);
        *self.last.lock().unwrap() = file.voice.clone();
        let result = if self.fail.swap(false, Ordering::SeqCst) {
            Err(attachments::Error::Upload)
        } else {
            Ok(crate::protocol_uploads::tests::prepared(&file, caption, d))
        };
        crate::attachment_worker::upload(control, async move { result }).await
    }
}
struct NoPicker;
#[async_trait::async_trait]
impl attachments::Picker for NoPicker {
    async fn open(
        &self,
        _: attachments::Kind,
        _: &str,
        _: &Control,
    ) -> Result<Vec<std::path::PathBuf>, attachments::Error> {
        panic!("recordings must not open a file chooser")
    }
}
async fn phase(status: &mut watch::Receiver<Status>, p: i32) {
    // Full-suite encryption tests share the bounded preparation permit.
    tokio::time::timeout(Duration::from_secs(if p == 0 { 30 } else { 5 }), async {
        loop {
            if status.borrow().phase == p {
                return;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn record_review_retry_handoff_queues_voice_without_changing_text_draft() {
    let (_dir, db) = crate::archive::tests::fixture();
    let chat = db
        .create_chat(crate::archive::tests::PN.into())
        .await
        .unwrap();
    db.draft(chat, "Keep the text draft".into()).await.unwrap();
    let (_archive, arx) = watch::channel(Some(db.clone()));
    let source = Arc::new(Fake {
        fail: AtomicBool::new(true),
        last: Mutex::new(None),
    });
    let (_sources, srx) = watch::channel(Some(source.clone() as Arc<dyn Uploader>));
    let (upload_tx, upload_rx) = mpsc::channel(16);
    let (upload_updates, mut upload_status) = watch::channel(attachments::Snapshot::default());
    let upload = tokio::spawn(attachments::run(
        arx.clone(),
        srx.clone(),
        upload_rx,
        upload_updates,
        Arc::new(NoPicker),
        false,
    ));
    let (tx, rx) = mpsc::channel(16);
    let (updates, mut status) = watch::channel(Status::default());
    let service = tokio::spawn(run(
        arx,
        srx,
        rx,
        updates,
        upload_tx.clone(),
        false,
        watch::channel(crate::preferences::Preferences::default()).1,
    ));
    tx.send(Command::Start {
        chat,
        title: "Captured recipient".into(),
    })
    .await
    .unwrap();
    phase(&mut status, 1).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while status.borrow().seconds < 0.2 {
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    tx.send(Command::Finish).await.unwrap();
    phase(&mut status, 4).await;
    tx.send(Command::Preview).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    tx.send(Command::Send).await.unwrap();
    phase(&mut status, 0).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while upload_status.borrow().phase != 6 {
            upload_status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(upload_status.borrow().voice);
    assert!(db.claim_next().await.unwrap().is_none());
    upload_tx
        .send(attachments::Command::Send(String::new()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while upload_status.borrow().phase != 0 {
            upload_status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let job = db.claim_next().await.unwrap().unwrap();
    assert!(job.raw.len() > 100);
    let request = db
        .call(|db| Ok(db.query_row("SELECT id FROM messages LIMIT 1", [], |r| r.get(0))?))
        .await
        .unwrap();
    let request = db.audio_request(request).await.unwrap().unwrap();
    let bytes = db.cached_audio(&request).await.unwrap().unwrap();
    assert_eq!(
        bytes.as_slice(),
        source.last.lock().unwrap().as_ref().unwrap().bytes.as_ref()
    );
    assert_eq!(
        db.view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .draft,
        "Keep the text draft"
    );
    drop(tx);
    tokio::time::timeout(Duration::from_secs(5), service)
        .await
        .unwrap()
        .unwrap();
    drop(upload_tx);
    upload.await.unwrap();
}
#[tokio::test]
async fn closing_an_active_recording_releases_capture_without_sending() {
    let (tx, rx) = mpsc::channel(16);
    let (updates, mut status) = watch::channel(Status::default());
    let (upload_tx, mut uploads) = mpsc::channel(16);
    let service = tokio::spawn(run(
        watch::channel(None).1,
        watch::channel(None).1,
        rx,
        updates,
        upload_tx,
        true,
        watch::channel(crate::preferences::Preferences::default()).1,
    ));
    tx.send(Command::Start {
        chat: 1,
        title: "Synthetic".into(),
    })
    .await
    .unwrap();
    phase(&mut status, 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(tx);
    tokio::time::timeout(Duration::from_secs(5), service)
        .await
        .unwrap()
        .unwrap();
    assert!(uploads.recv().await.is_none());
}

#[tokio::test]
async fn disconnect_finishes_capture_and_refused_handoff_retains_the_recording() {
    let (_dir, db) = crate::archive::tests::fixture();
    let chat = db
        .create_chat(crate::archive::tests::PN.into())
        .await
        .unwrap();
    let source = Arc::new(Fake {
        fail: AtomicBool::new(false),
        last: Mutex::new(None),
    });
    let (sources, srx) = watch::channel(Some(source.clone() as Arc<dyn Uploader>));
    let (_archive, arx) = watch::channel(Some(db));
    let (tx, rx) = mpsc::channel(16);
    let (updates, mut status) = watch::channel(Status::default());
    let (uploads, mut pending) = mpsc::channel(16);
    let service = tokio::spawn(run(
        arx,
        srx,
        rx,
        updates,
        uploads,
        false,
        watch::channel(crate::preferences::Preferences::default()).1,
    ));
    tx.send(Command::Start {
        chat,
        title: "Original recipient".into(),
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while status.borrow().seconds < 0.2 {
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    sources.send_replace(None);
    phase(&mut status, 4).await;
    tx.send(Command::Send).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !status.borrow().error.contains("Connect") {
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(pending.try_recv().is_err());
    sources.send_replace(Some(source as Arc<dyn Uploader>));
    tx.send(Command::Send).await.unwrap();
    let handoff = tokio::time::timeout(Duration::from_secs(30), pending.recv())
        .await
        .unwrap()
        .unwrap();
    let attachments::Command::Voice {
        destination,
        file,
        accepted,
    } = handoff
    else {
        panic!("expected recorded voice handoff")
    };
    assert_eq!(destination.chat, chat);
    assert_eq!(destination.title, "Original recipient");
    assert!(file.voice.is_some());
    accepted.send(Err(attachments::Error::Upload)).unwrap();
    phase(&mut status, 4).await;
    assert!(status.borrow().seconds > 0.);
    assert!(status.borrow().error.contains("current attachment"));
    tx.send(Command::Cancel).await.unwrap();
    phase(&mut status, 0).await;
    drop(tx);
    service.await.unwrap();
}
