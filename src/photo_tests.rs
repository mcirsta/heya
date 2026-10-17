use super::*;
use crate::{
    archive::{
        Batch, Change, Record,
        tests::{KEY, LID, PN, fixture, record},
    },
    message_content::Content,
};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Semaphore, mpsc};
use whatsapp_rust::{
    prelude::wa,
    waproto::{buffa::MessageField, codec},
};

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(w, h, image::Rgba([25, 150, 210, 255]))
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    output.into_inner()
}
fn envelope() -> wa::Message {
    let mut image = wa::message::ImageMessage::default();
    image.width = Some(1600);
    image.height = Some(1200);
    image.caption = Some("A synthetic photo 👩🏽‍💻\n\nKeep this caption in full.".into());
    image.jpeg_thumbnail = Some(png(80, 60));
    image.file_length = Some(12345);
    let mut message = wa::Message::default();
    message.image_message = MessageField::some(image);
    message
}
fn photo_record(id: &str) -> Record {
    let mut r = record(id);
    let message = envelope();
    let (_, content, thumbnail) = crate::protocol_content::extract(&message);
    r.content = content;
    r.thumbnail = thumbnail;
    r.raw = codec::message_to_vec(&message);
    r
}
async fn insert(db: &Archive, records: Vec<Record>) -> Vec<i32> {
    db.ingest(Batch {
        messages: records,
        ..Default::default()
    })
    .await
    .unwrap();
    db.blocking(|db| {
        Ok(db
            .prepare("SELECT id FROM messages WHERE visible=1 ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })
    .unwrap()
}
async fn cached(db: &Archive, id: i32, w: u32, h: u32) {
    let request = db.photo_request(id).await.unwrap().unwrap();
    assert!(
        db.save_photo(&request, prepare(png(w, h)).unwrap())
            .await
            .unwrap()
    );
}
async fn state(db: &Archive, id: i32, expected: State) {
    let mut changed = db.subscribe();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if db.photo_candidates(vec![id]).await.unwrap() == vec![(id, expected)] {
                return;
            }
            changed.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}
async fn snapshot(
    rx: &mut watch::Receiver<Arc<Snapshot>>,
    predicate: impl Fn(&Snapshot) -> bool,
) -> Arc<Snapshot> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let value = rx.borrow_and_update().clone();
            if predicate(&value) {
                return value;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}

#[test]
fn photo_metadata_wrappers_and_bounded_decode() {
    let message = codec::message_decode(&codec::message_to_vec(&envelope())).unwrap();
    let (_, content, thumbnail) = crate::protocol_content::extract(&message);
    assert_eq!(content.photo.as_ref().unwrap().width, 1600);
    assert_eq!(
        content.body.as_deref(),
        Some("A synthetic photo 👩🏽‍💻\n\nKeep this caption in full.")
    );
    assert!(!thumbnail.is_empty());
    let mut wrapper = wa::message::FutureProofMessage::default();
    wrapper.message = MessageField::some(message);
    let mut wrapped = wa::Message::default();
    wrapped.ephemeral_message = MessageField::some(wrapper.clone());
    assert_eq!(crate::protocol_content::extract(&wrapped).1, content);
    wrapped.ephemeral_message = Default::default();
    wrapped.view_once_message = MessageField::some(wrapper);
    assert_eq!(
        crate::protocol_content::extract(&wrapped).1,
        Content::default()
    );
    let old: Content =
        serde_json::from_str(r#"{"body":null,"preview":null,"actions":[]}"#).unwrap();
    assert!(old.photo.is_none());
    let mut malformed = envelope();
    let p = malformed.image_message.get_or_insert_default();
    p.width = Some(0);
    p.height = Some(u32::MAX);
    p.jpeg_thumbnail = Some(vec![0; crate::message_thumbnails::MAX_ENCODED + 1]);
    let (_, value, thumb) = crate::protocol_content::extract(&malformed);
    assert_eq!(
        (
            value.photo.as_ref().unwrap().width,
            value.photo.unwrap().height
        ),
        (640, 480)
    );
    assert!(thumb.is_empty());
    for (w, h, expected) in [
        (1600, 1200, (640, 480)),
        (900, 1800, (320, 640)),
        (2000, 500, (640, 160)),
    ] {
        let prepared = prepare(png(w, h)).unwrap();
        let decoded = decode(&prepared.preview, INLINE_EDGE).unwrap();
        assert_eq!((decoded.width, decoded.height), expected);
        assert_eq!(decoded.rgba.len(), (expected.0 * expected.1 * 4) as usize);
    }
    assert!(decode(b"not an image", INLINE_EDGE).is_none());
    assert!(decode(&vec![0; MAX_BYTES + 1], INLINE_EDGE).is_none());
    assert!(decode(&png(16385, 1), INLINE_EDGE).is_none());
    let d = Demand {
        visible: (1..100).collect(),
        viewer: Some(99),
        ..Default::default()
    };
    assert_eq!(d.ids().len(), MAX_DEMAND + 1);
    assert_eq!(d.ids()[0], 99);
}

#[tokio::test]
async fn photos_upgrade_reopen_encrypted_and_keep_exact_duplicate_cache() {
    use crate::storage::{prepare_at, tests::MemoryVault};
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let db = Archive::open(files.clone(), KEY).unwrap();
    let mut old = photo_record("old-photo");
    old.content = Default::default();
    old.thumbnail.clear();
    insert(&db, vec![old]).await;
    db.blocking(|c|{c.execute_batch("DROP TRIGGER discard_changed_document_preview; DROP TABLE document_previews; DROP TABLE outgoing_reads; DROP INDEX messages_action_wire; DROP TABLE reply_drafts; DROP TABLE reactions; DROP TRIGGER discard_changed_audio; DROP TABLE message_audio; DROP TRIGGER discard_changed_photo; DROP TABLE message_photos; DROP TABLE history_pages; DROP TABLE history_requests; ALTER TABLE messages DROP COLUMN edit_version; ALTER TABLE mutations DROP COLUMN raw; PRAGMA user_version=5; INSERT OR REPLACE INTO metadata(key,value) VALUES('content_decoder_revision','1');")?;Ok(())}).unwrap();
    drop(db);
    let db = Archive::open(files.clone(), KEY).unwrap();
    crate::protocol_messages::repair_text(&db).unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert!(view.messages[0].content.photo.is_some());
    let mut alias = photo_record("old-photo");
    alias.chat = LID.into();
    alias.sender = LID.into();
    let ids = insert(&db, vec![alias]).await;
    let in_flight = db.photo_request(ids[0]).await.unwrap().unwrap();
    db.photo_state(&in_flight, State::Loading).await.unwrap();
    cached(&db, ids[1], 1600, 1200).await;
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    let rows = db
        .photo_rows(vec![ids[0]], BTreeMap::new(), Some((ids[0], None)))
        .await
        .unwrap();
    assert_eq!(rows[0].state, State::Ready);
    assert!(
        !db.photo_state(&in_flight, State::Failed).await.unwrap(),
        "a late download failure cannot demote the verified merged cache"
    );
    assert!(rows[0].original.as_ref().is_some_and(|v| !v.is_empty()));
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert_eq!(
        db.photo_candidates(vec![ids[0]]).await.unwrap(),
        vec![(ids[0], State::Ready)]
    );
    drop(db);
    let disk = std::fs::read(temp.path().join("archive.db")).unwrap();
    assert!(!disk.starts_with(b"SQLite format"));
    assert!(
        !disk
            .windows(b"synthetic photo".len())
            .any(|b| b == b"synthetic photo")
    );
    assert!(!disk.windows(8).any(|b| b == b"\x89PNG\r\n\x1a\n"));
}

#[tokio::test]
async fn caption_edits_keep_photos_but_deletion_expiry_and_stale_envelopes_cannot_restore_them() {
    let (_temp, db) = fixture();
    let ids = insert(
        &db,
        vec![
            photo_record("edited"),
            photo_record("revoked"),
            photo_record("expired"),
            photo_record("replaced"),
        ],
    )
    .await;
    let mut requests = Vec::new();
    for &id in &ids {
        requests.push(db.photo_request(id).await.unwrap().unwrap());
        cached(&db, id, 80, 60).await;
    }
    for (index, text) in [(0, Some("Edited caption".into())), (1, None)] {
        let mut change = record(&format!("change-{index}"));
        change.visible = false;
        change.change = Some(Change {
            id: ["edited", "revoked"][index].into(),
            sender: PN.into(),
            text,
            version: 1,
            raw: Vec::new(),
        });
        db.ingest(Batch {
            messages: vec![change],
            ..Default::default()
        })
        .await
        .unwrap();
    }
    db.blocking(|c| {
        c.execute("UPDATE messages SET expires=1 WHERE wire_id='expired'", [])?;
        c.execute(
            "UPDATE messages SET raw=X'FFFF' WHERE wire_id='replaced'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(
        db.save_photo(&requests[0], prepare(png(80, 60)).unwrap())
            .await
            .unwrap()
    );
    for request in &requests[1..] {
        assert!(
            !db.save_photo(request, prepare(png(80, 60)).unwrap())
                .await
                .unwrap()
        );
        assert!(!db.photo_state(request, State::Loading).await.unwrap());
    }
    let chat = db.create_chat(PN.into()).await.unwrap();
    db.view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    db.blocking(|c|{let count:i32=c.query_row("SELECT count(*) FROM message_photos WHERE message_id IN (SELECT id FROM messages WHERE wire_id IN ('edited','revoked','expired'))",[],|r|r.get(0))?;assert_eq!(count,1);Ok(())}).unwrap();
    assert!(
        db.photo_candidates(ids[1..3].to_vec())
            .await
            .unwrap()
            .is_empty()
    );
}

struct Fake {
    started: mpsc::UnboundedSender<()>,
    gate: Semaphore,
    result: Mutex<Result<Vec<u8>, State>>,
    cancelled: Arc<AtomicUsize>,
}
struct InFlight {
    cancelled: Arc<AtomicUsize>,
    done: bool,
}
impl Drop for InFlight {
    fn drop(&mut self) {
        if !self.done {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
        }
    }
}
#[async_trait::async_trait]
impl Source for Arc<Fake> {
    async fn fetch(&self, _: &[u8]) -> Result<Vec<u8>, State> {
        let mut guard = InFlight {
            cancelled: self.cancelled.clone(),
            done: false,
        };
        self.started.send(()).unwrap();
        self.gate.acquire().await.unwrap().forget();
        guard.done = true;
        self.result.lock().unwrap().clone()
    }
}
async fn started(rx: &mut mpsc::UnboundedReceiver<()>) {
    tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn photo_downloads_obey_visibility_preference_offline_retry_and_cancellation() {
    let (_temp, db) = fixture();
    let ids = insert(&db, vec![photo_record("a"), photo_record("b")]).await;
    let (tx, mut calls) = mpsc::unbounded_channel();
    let source = Arc::new(Fake {
        started: tx,
        gate: Semaphore::new(0),
        result: Mutex::new(Err(State::Failed)),
        cancelled: Default::default(),
    });
    let (demand, demand_rx) = watch::channel(Demand {
        visible: ids.clone(),
        automatic: false,
        ..Default::default()
    });
    let (online, online_rx) = watch::channel(false);
    let (stop, stop_rx) = watch::channel(false);
    let worker = tokio::spawn(fetch_worker(
        db.clone(),
        source.clone(),
        demand_rx,
        online_rx,
        stop_rx,
    ));
    online.send_replace(true);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), calls.recv())
            .await
            .is_err()
    );
    demand.send_modify(|d| d.retry = Some((ids[0], 1)));
    started(&mut calls).await;
    source.gate.add_permits(1);
    state(&db, ids[0], State::Failed).await;
    demand.send_modify(|d| d.automatic = true);
    // Failed photo is not retried automatically; the second visible photo starts.
    started(&mut calls).await;
    state(&db, ids[1], State::Loading).await;
    demand.send_modify(|d| d.visible = vec![ids[0]]);
    state(&db, ids[1], State::Idle).await;
    assert_eq!(source.cancelled.load(Ordering::SeqCst), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), calls.recv())
            .await
            .is_err()
    );
    *source.result.lock().unwrap() = Ok(png(1600, 1200));
    online.send_replace(false);
    demand.send_modify(|d| d.retry = Some((ids[0], 2)));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), calls.recv())
            .await
            .is_err()
    );
    online.send_replace(true);
    started(&mut calls).await;
    demand.send_modify(|d| d.retry = Some((ids[0], 3)));
    source.gate.add_permits(1);
    state(&db, ids[0], State::Ready).await;
    // A completed cached photo never downloads just because a receipt arrives.
    db.changed();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), calls.recv())
            .await
            .is_err()
    );
    demand.send_modify(|d| d.visible = vec![ids[1]]);
    started(&mut calls).await;
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(15), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(source.cancelled.load(Ordering::SeqCst), 2);
    assert_eq!(
        db.photo_candidates(vec![ids[1]]).await.unwrap(),
        vec![(ids[1], State::Idle)]
    );
}

#[tokio::test]
async fn cached_photo_presentation_reuses_evicts_bounds_and_loads_only_one_viewer() {
    let (_temp, db) = fixture();
    let ids = insert(
        &db,
        (0..40)
            .map(|i| photo_record(&format!("photo-{i}")))
            .collect(),
    )
    .await;
    cached(&db, ids[0], 2500, 1875).await;
    let (archives, archive_rx) = watch::channel(Some(db.clone()));
    let (demand, demand_rx) = watch::channel(Demand {
        visible: ids.clone(),
        ..Default::default()
    });
    let (updates, mut snapshots) = watch::channel(Arc::new(Snapshot::default()));
    let worker = tokio::spawn(present(archive_rx, demand_rx, updates));
    let first = snapshot(&mut snapshots, |s| {
        s.rows.len() == MAX_DEMAND && s.rows.values().all(|p| p.image.is_some())
    })
    .await;
    assert_eq!(first.rows[&ids[0]].image.as_ref().unwrap().width, 640);
    assert!(first.viewer.is_none());
    db.changed();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), snapshots.changed())
            .await
            .is_err()
    );
    demand.send_modify(|d| d.viewer = Some(ids[0]));
    let full = snapshot(&mut snapshots, |s| s.viewer.is_some()).await;
    assert_eq!(full.viewer.as_ref().unwrap().1.width, 2048);
    assert!(Arc::ptr_eq(
        first.rows[&ids[0]].image.as_ref().unwrap(),
        full.rows[&ids[0]].image.as_ref().unwrap()
    ));
    demand.send_modify(|d| {
        d.viewer = Some(ids[39]);
        d.visible = vec![ids[0]];
    });
    let moved = snapshot(&mut snapshots, |s| s.rows.len() == 2 && s.viewer.is_none()).await;
    assert!(moved.rows.contains_key(&ids[39]));
    let request = db.photo_request(ids[39]).await.unwrap().unwrap();
    db.save_photo(
        &request,
        Cached {
            original: b"broken".to_vec(),
            preview: png(80, 60),
        },
    )
    .await
    .unwrap();
    snapshot(&mut snapshots, |s| {
        s.rows
            .get(&ids[39])
            .is_some_and(|p| p.state == State::Invalid)
    })
    .await;
    demand.send_replace(Demand::default());
    snapshot(&mut snapshots, |s| s.rows.is_empty() && s.viewer.is_none()).await;
    demand.send_modify(|d| d.visible = vec![ids[0]]);
    let back = snapshot(&mut snapshots, |s| {
        s.rows.get(&ids[0]).is_some_and(|p| p.image.is_some())
    })
    .await;
    assert!(!Arc::ptr_eq(
        first.rows[&ids[0]].image.as_ref().unwrap(),
        back.rows[&ids[0]].image.as_ref().unwrap()
    ));
    archives.send_replace(None);
    snapshot(&mut snapshots, |s| s.rows.is_empty()).await;
    drop(demand);
    tokio::time::timeout(Duration::from_secs(15), worker)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn jpeg_orientation_is_applied_once_to_inline_and_viewer_pixels() {
    let mut jpeg = Cursor::new(Vec::new());
    image::RgbImage::from_pixel(400, 200, image::Rgb([25, 150, 210]))
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    let jpeg = jpeg.into_inner();
    // Minimal little-endian EXIF IFD: orientation 6 (90 degrees clockwise).
    let exif=b"Exif\0\0II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00";
    let mut oriented = jpeg[..2].to_vec();
    oriented.extend_from_slice(&[0xff, 0xe1]);
    oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    oriented.extend_from_slice(exif);
    oriented.extend_from_slice(&jpeg[2..]);
    let cached = prepare(oriented.clone()).unwrap();
    assert_eq!(cached.original, oriented);
    for (bytes, edge) in [
        (&cached.preview, INLINE_EDGE),
        (&cached.original, VIEWER_EDGE),
    ] {
        let pixels = decode(bytes, edge).unwrap();
        assert_eq!((pixels.width, pixels.height), (200, 400));
        assert!(pixels.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }
}

#[tokio::test]
async fn a_slow_photo_does_not_block_ready_photos_or_publish_obsolete_pixels() {
    let (_temp, db) = fixture();
    let ids = insert(
        &db,
        (0..4)
            .map(|i| photo_record(&format!("progress-{i}")))
            .collect(),
    )
    .await;
    let (_archives, archive_rx) = watch::channel(Some(db.clone()));
    let (demand, demand_rx) = watch::channel(Demand {
        visible: ids[..3].to_vec(),
        ..Default::default()
    });
    let (updates, mut snapshots) = watch::channel(Arc::new(Snapshot::default()));
    let gate = Arc::new(Semaphore::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let decode = {
        let gate = gate.clone();
        let calls = calls.clone();
        move |bytes: Vec<u8>, edge| {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            let gate = gate.clone();
            let started = started_tx.clone();
            async move {
                started.send(call).unwrap();
                if call == 1 {
                    gate.acquire().await.unwrap().forget();
                }
                super::decode(&bytes, edge)
            }
        }
    };
    let worker = tokio::spawn(presentation::present_with_decode(
        archive_rx, demand_rx, updates, decode,
    ));
    assert_eq!(started_rx.recv().await, Some(0));
    assert_eq!(started_rx.recv().await, Some(1));
    // The second image is deliberately held: the first must already be visible.
    let first = snapshot(&mut snapshots, |s| {
        s.rows.get(&ids[0]).is_some_and(|p| p.image.is_some())
    })
    .await;
    assert!(first.rows[&ids[1]].image.is_none());
    // Scroll back and forth while it decodes; retained images must not be
    // discarded just because demand changed. Revoke the in-flight photo too.
    demand.send_modify(|d| d.visible = vec![ids[0], ids[1], ids[3]]);
    db.call({
        let id = ids[1];
        move |db| {
            db.execute("UPDATE messages SET deleted=1 WHERE id=?1", [id])?;
            Ok(())
        }
    })
    .await
    .unwrap();
    db.changed();
    gate.add_permits(1);
    let last = snapshot(&mut snapshots, |s| {
        s.rows.len() == 2 && s.rows.get(&ids[3]).is_some_and(|p| p.image.is_some())
    })
    .await;
    assert!(!last.rows.contains_key(&ids[1]));
    assert!(!last.rows.contains_key(&ids[2]));
    assert!(Arc::ptr_eq(
        first.rows[&ids[0]].image.as_ref().unwrap(),
        last.rows[&ids[0]].image.as_ref().unwrap()
    ));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "skip the offscreen row and reuse the overlapping photo"
    );
    drop(demand);
    tokio::time::timeout(Duration::from_secs(15), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn attachment_menu_upgrade_preserves_cached_photo_fingerprint() {
    let (_temp, db) = fixture();
    let id = insert(&db, vec![photo_record("menu-upgrade")]).await[0];
    cached(&db, id, 160, 120).await;
    db.blocking(|c| {
        c.execute(
            "INSERT OR REPLACE INTO metadata(key,value) VALUES('content_decoder_revision','4')",
            [],
        )?;
        let content: String =
            c.query_row("SELECT content FROM messages LIMIT 1", [], |r| r.get(0))?;
        assert!(
            !content.contains("has_attachment"),
            "existing photo JSON must remain byte-compatible"
        );
        Ok(())
    })
    .unwrap();
    crate::protocol_messages::repair_text(&db).unwrap();
    assert_eq!(
        db.photo_candidates(vec![id]).await.unwrap(),
        vec![(id, State::Ready)]
    );
}
