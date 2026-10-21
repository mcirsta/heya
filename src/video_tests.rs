use super::*;
use crate::archive::{
    Batch,
    tests::{LID, PN, fixture, record},
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn video_record(id: &str) -> crate::archive::Record {
    let mut row = record(id);
    row.content.video = Some(crate::message_content::Video {
        width: 1280,
        height: 720,
        seconds: Some(4),
        size: Some(10),
        gif: false,
    });
    row.content.has_attachment = true;
    row
}
struct Bytes(AtomicUsize);
#[async_trait::async_trait]
impl Source for Bytes {
    async fn fetch(&self, _: &[u8]) -> Result<Vec<u8>, Error> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(b"synthetic video original".to_vec())
    }
}

#[tokio::test]
async fn encrypted_video_cache_is_lazy_reusable_and_fences_removed_content() {
    let (_temp, db) = fixture();
    db.ingest(Batch {
        messages: vec![video_record("sample")],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let id = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
        .messages[0]
        .id;
    let source = Arc::new(Bytes(AtomicUsize::new(0)));
    let (request, bytes) = if SUPPORTED {
        let result = load(Some(db.clone()), Some(source.clone()), id, false)
            .await
            .unwrap();
        assert_eq!(source.0.load(Ordering::Relaxed), 1);
        assert_eq!(
            load(Some(db.clone()), None, id, false).await.unwrap().1,
            result.1,
            "cached video plays offline"
        );
        result
    } else {
        assert!(matches!(
            load(Some(db.clone()), Some(source.clone()), id, false).await,
            Err(Error::NotBuilt)
        ));
        assert_eq!(source.0.load(Ordering::Relaxed), 0);
        let request = db.video_request(id).await.unwrap().unwrap();
        let bytes = b"synthetic video original".to_vec();
        db.save_video(&request, bytes.clone()).await.unwrap();
        (Some(request), bytes)
    };
    assert_eq!(
        db.download_cached(
            &crate::downloads::Request {
                chat,
                id,
                raw: request.as_ref().unwrap().raw.clone()
            },
            crate::downloads::Kind::Video
        )
        .await
        .unwrap()
        .unwrap(),
        bytes
    );
    let request = request.unwrap();
    db.blocking(|c| {
        c.execute(
            "UPDATE messages SET deleted=1,body='This message was deleted'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(db.video_request(id).await.unwrap().is_none());
    assert!(
        !db.save_video(&request, bytes).await.unwrap(),
        "late download cannot restore a revoked video"
    );
    assert!(db.cached_video(&request).await.unwrap().is_none());
}

#[tokio::test]
async fn video_cache_follows_alias_merges_and_lock_guards() {
    let (_temp, db) = fixture();
    let mut lid = video_record("sample");
    lid.chat = LID.into();
    lid.sender = LID.into();
    let pn = video_record("sample");
    db.ingest(Batch {
        messages: vec![lid, pn],
        ..Default::default()
    })
    .await
    .unwrap();
    let (lid_id, pn_id) = db
        .blocking(|c| {
            Ok((
                c.query_row("SELECT id FROM messages WHERE sender=?1", [LID], |r| {
                    r.get::<_, i32>(0)
                })?,
                c.query_row("SELECT id FROM messages WHERE sender=?1", [PN], |r| {
                    r.get::<_, i32>(0)
                })?,
            ))
        })
        .unwrap();
    let request = db.video_request(lid_id).await.unwrap().unwrap();
    db.save_video(&request, b"synthetic bytes".to_vec())
        .await
        .unwrap();
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    let request = db.video_request(pn_id).await.unwrap().unwrap();
    assert_eq!(
        db.cached_video(&request).await.unwrap().unwrap(),
        b"synthetic bytes"
    );
    db.save_chat_settings(vec![
        crate::chat_management::Action::Lock.update(PN.into(), 1),
    ])
    .await
    .unwrap();
    assert!(db.video_request(pn_id).await.unwrap().is_none());
    assert!(db.cached_video(&request).await.unwrap().is_none());
    assert!(
        !db.save_video(&request, b"late bytes".to_vec())
            .await
            .unwrap()
    );
}

#[test]
fn protocol_video_projection_preserves_caption_and_excludes_view_once_previews() {
    use whatsapp_rust::prelude::wa;
    let mut clip = wa::message::VideoMessage::default();
    clip.caption = Some("A whole caption 👩🏽‍💻\nSecond line".into());
    clip.width = Some(1280);
    clip.height = Some(720);
    clip.seconds = Some(4);
    clip.jpeg_thumbnail = Some(vec![1, 2, 3]);
    let mut message = wa::Message::default();
    message.video_message = clip.clone().into();
    let (_, content, poster) = crate::protocol_content::extract(&message);
    assert_eq!(content.body, clip.caption);
    assert_eq!(content.video.unwrap().seconds, Some(4));
    assert_eq!(poster, vec![1, 2, 3]);
    clip.view_once = Some(true);
    message.video_message = clip.into();
    let (_, content, poster) = crate::protocol_content::extract(&message);
    assert!(content.video.is_none());
    assert!(poster.is_empty());
}

#[tokio::test]
async fn cancelling_a_download_and_closing_the_service_drop_the_fetch() {
    if !SUPPORTED {
        return;
    }
    struct Pending {
        began: tokio::sync::Notify,
        cancelled: Arc<AtomicUsize>,
    }
    struct Fetch(Arc<AtomicUsize>);
    impl Drop for Fetch {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    #[async_trait::async_trait]
    impl Source for Pending {
        async fn fetch(&self, _: &[u8]) -> Result<Vec<u8>, Error> {
            let _fetch = Fetch(self.cancelled.clone());
            self.began.notify_one();
            std::future::pending().await
        }
    }
    let (_temp, db) = fixture();
    db.ingest(Batch {
        messages: vec![video_record("pending")],
        ..Default::default()
    })
    .await
    .unwrap();
    let id = db
        .blocking(|c| Ok(c.query_row("SELECT id FROM messages", [], |r| r.get::<_, i32>(0))?))
        .unwrap();
    let cancelled = Arc::new(AtomicUsize::new(0));
    let source = Arc::new(Pending {
        began: Default::default(),
        cancelled: cancelled.clone(),
    });
    let (_archives, archives) = watch::channel(Some(db));
    let (_sources, sources) = watch::channel(Some(source.clone() as Arc<dyn Source>));
    let (commands, rx) = mpsc::channel(4);
    let (updates, mut states) = watch::channel(Status::default());
    let task = tokio::spawn(run(
        archives,
        sources,
        rx,
        updates,
        false,
        watch::channel(Default::default()).1,
    ));
    commands.send(Command::Toggle(id)).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), source.began.notified())
        .await
        .unwrap();
    commands.send(Command::Stop).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        states.wait_for(|s| s.id == 0),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cancelled.load(Ordering::Relaxed), 1);
    commands.send(Command::Toggle(id)).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), source.began.notified())
        .await
        .unwrap();
    drop(commands);
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancelled.load(Ordering::Relaxed), 2);
}
