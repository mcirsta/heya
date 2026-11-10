use super::*;
use crate::{
    archive::{
        Batch,
        tests::{fixture, record},
    },
    message_content::{Content, Document},
    message_thumbnails,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::mpsc;

fn png(image: image::RgbaImage) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}
fn sharp() -> Vec<u8> {
    // Fine alternating rules above a differently coloured lower page: cropping
    // and preserved source resolution are objectively testable, without a PDF.
    png(image::RgbaImage::from_fn(WIDTH, 1024, |x, y| {
        if y >= HEIGHT {
            image::Rgba([200, 0, 0, 255])
        } else if x % 2 == 0 {
            image::Rgba([0, 0, 0, 255])
        } else {
            image::Rgba([255, 255, 255, 255])
        }
    }))
}
fn tiny() -> Vec<u8> {
    png(image::RgbaImage::from_pixel(
        32,
        45,
        image::Rgba([128, 128, 128, 255]),
    ))
}

#[tokio::test]
async fn video_posters_keep_aspect_upgrade_once_and_never_populate_the_original_cache() {
    let (_temp, db) = fixture();
    let mut r = record("synthetic-video-poster");
    r.thumbnail = tiny();
    r.content.video = Some(crate::message_content::Video {
        round: false,
        width: 720,
        height: 1280,
        seconds: Some(10),
        size: Some(123456),
        gif: false,
    });
    db.ingest(Batch {
        messages: vec![r],
        ..Default::default()
    })
    .await
    .unwrap();
    let id = db
        .blocking(|db| Ok(db.query_row("SELECT id FROM messages", [], |r| r.get(0))?))
        .unwrap();
    let (archives, archive_rx) = watch::channel(Some(db.clone()));
    let (demand, demand_rx) = watch::channel(vec![id]);
    let (updates, mut images) = watch::channel(message_thumbnails::Snapshot::default());
    let worker = tokio::spawn(message_thumbnails::present(archive_rx, demand_rx, updates));
    changed(&mut images).await;
    let low = images.borrow_and_update().clone();
    assert_eq!(low[&id].width, 32);
    let request = db
        .document_preview_request(vec![id])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.kind, Kind::Video);
    let pixels = video_decode(&png(image::RgbaImage::from_pixel(
        720,
        1280,
        image::Rgba([90, 150, 200, 255]),
    )))
    .unwrap();
    assert_eq!(
        (pixels.width, pixels.height),
        (270, 480),
        "video posters aren't cropped into PDF strips"
    );
    db.save_document_preview(&request, Some(encode(pixels).unwrap()))
        .await
        .unwrap();
    changed(&mut images).await;
    let high = images.borrow_and_update().clone();
    assert_eq!((high[&id].width, high[&id].height), (270, 480));
    assert!(!Arc::ptr_eq(&low[&id], &high[&id]));
    assert!(
        db.document_preview_request(vec![id])
            .await
            .unwrap()
            .is_none()
    );
    db.changed();
    assert!(
        tokio::time::timeout(Duration::from_millis(120), images.changed())
            .await
            .is_err()
    );
    db.blocking(|db| {
        assert_eq!(
            db.query_row("SELECT count(*) FROM message_videos", [], |r| r
                .get::<_, i32>(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
    db.call(move |db| {
        db.execute(
            "UPDATE messages SET deleted=1,content='',raw=X'',thumbnail=X'' WHERE id=?1",
            [id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    db.changed();
    assert!(
        !db.save_document_preview(&request, Some(tiny()))
            .await
            .unwrap()
    );
    changed(&mut images).await;
    assert!(images.borrow_and_update().is_empty());
    drop(demand);
    worker.await.unwrap();
    drop(archives);
}

#[tokio::test]
async fn a_new_verified_original_retries_a_failed_poster_once_without_a_retry_loop() {
    let (_temp, db) = fixture();
    let mut r = record("synthetic-video-retry");
    r.content.video = Some(crate::message_content::Video {
        round: false,
        width: 1280,
        height: 720,
        seconds: Some(4),
        size: Some(123456),
        gif: false,
    });
    db.ingest(Batch {
        messages: vec![r],
        ..Default::default()
    })
    .await
    .unwrap();
    let id = db
        .blocking(|db| Ok(db.query_row("SELECT id FROM messages", [], |r| r.get(0))?))
        .unwrap();
    let request = db
        .document_preview_request(vec![id])
        .await
        .unwrap()
        .unwrap();
    db.save_document_preview(&request, None).await.unwrap();
    assert!(
        db.document_preview_request(vec![id])
            .await
            .unwrap()
            .is_none()
    );
    let original = db.video_request(id).await.unwrap().unwrap();
    assert!(db.save_video(&original, vec![42; 128]).await.unwrap());
    assert!(
        db.document_preview_request(vec![id])
            .await
            .unwrap()
            .is_some()
    );
    db.save_document_preview(&request, None).await.unwrap();
    assert!(
        db.document_preview_request(vec![id])
            .await
            .unwrap()
            .is_none(),
        "cached originals must not bypass failure cooldown on every archive update"
    );
}

#[tokio::test]
async fn low_resolution_cache_is_upgraded_and_failed_attempts_keep_old_pixels() {
    let (_temp, db) = fixture();
    let id = seed(&db).await[0];
    let request = db
        .document_preview_request(vec![id])
        .await
        .unwrap()
        .unwrap();
    db.save_document_preview(&request, Some(prepare(tiny()).unwrap()))
        .await
        .unwrap();
    let old = db
        .blocking(|c| {
            Ok(
                c.query_row("SELECT id,preview FROM document_previews", [], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
                })?,
            )
        })
        .unwrap();
    db.save_document_preview(&request, None).await.unwrap();
    db.blocking(|c| {
        let saved = c.query_row("SELECT id,preview FROM document_previews", [], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        assert_eq!(
            saved, old,
            "failure must retain both the pixels and their cache identity"
        );
        c.execute("UPDATE document_previews SET retry_at=0", [])?;
        Ok(())
    })
    .unwrap();
    assert!(
        db.document_preview_request(vec![id])
            .await
            .unwrap()
            .is_some()
    );
    db.save_document_preview(&request, Some(prepare(sharp()).unwrap()))
        .await
        .unwrap();
    db.blocking(|c| {
        let (revision, sharp) = c.query_row("SELECT id,sharp FROM document_previews", [], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?))
        })?;
        assert!(revision > old.0);
        assert!(sharp);
        Ok(())
    })
    .unwrap();
    assert!(
        db.document_preview_request(vec![id])
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !db.save_document_preview(&request, None).await.unwrap(),
        "do not downgrade a completed preview"
    );
}
async fn seed(db: &Archive) -> Vec<i32> {
    let messages = (1..=2)
        .map(|n| {
            let mut r = record(&format!("synthetic-doc-{n}"));
            r.raw = vec![n];
            r.thumbnail = tiny();
            r.content = Content {
                document: Some(Document {
                    file_name: "Synthetic.pdf".into(),
                    mime_type: "application/pdf".into(),
                    size: Some(10000),
                    pages: Some(2),
                    has_thumbnail: true,
                }),
                ..Default::default()
            };
            r
        })
        .collect();
    db.ingest(Batch {
        messages,
        ..Default::default()
    })
    .await
    .unwrap();
    db.blocking(|c| {
        Ok(c.prepare("SELECT id FROM messages ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })
    .unwrap()
}
async fn changed<T>(rx: &mut watch::Receiver<T>) {
    tokio::time::timeout(Duration::from_secs(10), rx.changed())
        .await
        .unwrap()
        .unwrap();
}
#[test]
fn document_strip_preserves_fine_detail_at_hidpi_without_retaining_the_full_page() {
    let bytes = sharp();
    let p = decode(&bytes).unwrap();
    assert_eq!((p.width, p.height), (WIDTH, HEIGHT));
    assert_eq!(&p.rgba[..8], &[0, 0, 0, 255, 255, 255, 255, 255]);
    assert_eq!(p.rgba.len(), (WIDTH * HEIGHT * 4) as usize);
    let saved = prepare(bytes).unwrap();
    assert_eq!(decode(&saved).unwrap().rgba, p.rgba);
    let small = decode(&tiny()).unwrap();
    assert_eq!(small.width, 32, "do not manufacture detail by upscaling");
    assert!(decode(b"%PDF-1.7 synthetic invalid input").is_none());
    assert!(decode(&vec![0; MAX_BYTES + 1]).is_none());
    assert!(decode(&png(image::RgbaImage::new(4097, 1))).is_none());
}
#[tokio::test]
async fn upgrades_replace_cached_pixels_once_and_survive_presenter_restart() {
    let (_temp, db) = fixture();
    let ids = seed(&db).await;
    let (archives, archive_rx) = watch::channel(Some(db.clone()));
    let (demand, demand_rx) = watch::channel(vec![ids[0]]);
    let (updates, mut snapshots) = watch::channel(message_thumbnails::Snapshot::default());
    let worker = tokio::spawn(message_thumbnails::present(archive_rx, demand_rx, updates));
    changed(&mut snapshots).await;
    let low = snapshots.borrow_and_update().clone();
    assert_eq!(low[&ids[0]].width, 32);
    let request = db
        .document_preview_request(vec![ids[0]])
        .await
        .unwrap()
        .unwrap();
    assert!(
        db.save_document_preview(&request, Some(prepare(sharp()).unwrap()))
            .await
            .unwrap()
    );
    changed(&mut snapshots).await;
    let high = snapshots.borrow_and_update().clone();
    assert_eq!(high[&ids[0]].width, WIDTH);
    assert!(!Arc::ptr_eq(&low[&ids[0]], &high[&ids[0]]));
    db.changed();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), snapshots.changed())
            .await
            .is_err(),
        "receipts must not cause image uploads"
    );
    assert!(
        db.document_preview_request(vec![ids[0]])
            .await
            .unwrap()
            .is_none()
    );
    drop(demand);
    worker.await.unwrap();
    drop(archives);
    let (_archives, archive_rx) = watch::channel(Some(db.clone()));
    let (demand, demand_rx) = watch::channel(vec![ids[0]]);
    let (updates, mut snapshots) = watch::channel(message_thumbnails::Snapshot::default());
    let worker = tokio::spawn(message_thumbnails::present(archive_rx, demand_rx, updates));
    changed(&mut snapshots).await;
    assert_eq!(
        snapshots.borrow_and_update()[&ids[0]].width,
        WIDTH,
        "offline cache works without a source"
    );
    let id = ids[0];
    db.call(move |c| {
        c.execute(
            "UPDATE messages SET deleted=1,thumbnail=X'',content='' WHERE id=?1",
            [id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    db.changed();
    assert!(
        !db.save_document_preview(&request, Some(prepare(sharp()).unwrap()))
            .await
            .unwrap(),
        "late download cannot restore a deleted preview"
    );
    changed(&mut snapshots).await;
    assert!(snapshots.borrow_and_update().is_empty());
    drop(demand);
    worker.await.unwrap();
}
#[tokio::test]
async fn cache_bounds_failures_mutations_expiry_and_demand() {
    let (_temp, db) = fixture();
    let ids = seed(&db).await;
    let request = db
        .document_preview_request(vec![ids[0]])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        db.save_document_preview(&request, Some(vec![0; MAX_BYTES + 1]))
            .await,
        Err(archive::Error::Limit)
    );
    db.save_document_preview(&request, None).await.unwrap();
    assert!(
        db.document_preview_request(vec![ids[0]])
            .await
            .unwrap()
            .is_none(),
        "failure cooldown persists across worker restarts"
    );
    let mut bounded = vec![-1; MAX_DEMAND];
    bounded.push(ids[1]);
    assert!(
        db.document_preview_request(bounded)
            .await
            .unwrap()
            .is_none()
    );
    let request = db
        .document_preview_request(vec![ids[1]])
        .await
        .unwrap()
        .unwrap();
    let id = ids[1];
    db.call(move |c| {
        c.execute("UPDATE messages SET raw=X'99' WHERE id=?1", [id])?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(!db.document_preview_current(&request).await.unwrap());
    assert!(
        !db.save_document_preview(&request, Some(prepare(sharp()).unwrap()))
            .await
            .unwrap()
    );
    let request = db
        .document_preview_request(vec![id])
        .await
        .unwrap()
        .unwrap();
    db.call(move |c| {
        c.execute("UPDATE messages SET expires=1 WHERE id=?1", [id])?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(!db.document_preview_current(&request).await.unwrap());
    assert!(
        !db.save_document_preview(&request, Some(prepare(sharp()).unwrap()))
            .await
            .unwrap()
    );
}
struct Fake {
    calls: mpsc::UnboundedSender<u8>,
    gate: watch::Receiver<bool>,
    active: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl Source for Fake {
    async fn fetch(&self, request: &Request) -> Option<Vec<u8>> {
        struct Active(Arc<AtomicUsize>);
        impl Drop for Active {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        assert_eq!(
            self.active.fetch_add(1, Ordering::SeqCst),
            0,
            "one fetch at a time"
        );
        let _active = Active(self.active.clone());
        self.calls.send(request.raw[0]).unwrap();
        let mut gate = self.gate.clone();
        while !*gate.borrow_and_update() {
            gate.changed().await.ok()?;
        }
        Some(sharp())
    }
}
#[tokio::test]
async fn worker_cancels_obsolete_work_and_never_fetches_offline_or_cached_previews() {
    let (_temp, db) = fixture();
    let ids = seed(&db).await;
    let (calls, mut received) = mpsc::unbounded_channel();
    let (gate, gate_rx) = watch::channel(false);
    let active = Arc::new(AtomicUsize::new(0));
    let (demand, demand_rx) = watch::channel(vec![ids[0]]);
    let (online, online_rx) = watch::channel(false);
    let (stop, stop_rx) = watch::channel(false);
    let worker = tokio::spawn(fetch_worker(
        db.clone(),
        Fake {
            calls,
            gate: gate_rx,
            active: active.clone(),
        },
        demand_rx,
        online_rx,
        stop_rx,
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), received.recv())
            .await
            .is_err()
    );
    online.send_replace(true);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), received.recv())
            .await
            .unwrap(),
        Some(1)
    );
    demand.send_replace(vec![ids[1]]);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), received.recv())
            .await
            .unwrap(),
        Some(2)
    );
    let mut changes = db.subscribe();
    gate.send_replace(true);
    changed(&mut changes).await;
    assert!(
        db.document_preview_request(vec![ids[1]])
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db.document_preview_request(vec![ids[0]])
            .await
            .unwrap()
            .is_some(),
        "cancelled work remains eligible"
    );
    db.changed();
    online.send_replace(false);
    online.send_replace(true);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), received.recv())
            .await
            .is_err()
    );
    gate.send_replace(false);
    demand.send_replace(vec![ids[0]]);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), received.recv())
            .await
            .unwrap(),
        Some(1)
    );
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
}
