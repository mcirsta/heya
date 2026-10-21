use super::*;
use crate::archive::tests::{PN, fixture};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};

pub(crate) fn photo(value: u8) -> Thumbnail {
    Thumbnail::from_pixels(vec![value; (EDGE * EDGE * 4) as usize]).unwrap()
}
fn picture(value: u8) -> Outcome {
    Outcome::Photo {
        id: format!("synthetic-{value}"),
        photo: photo(value),
    }
}
fn png(image: image::RgbaImage) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[test]
fn decoding_crops_and_bounds_untrusted_images() {
    let image = image::RgbaImage::from_fn(64, 32, |x, _| {
        if (16..48).contains(&x) {
            image::Rgba([0, 0, 255, 255])
        } else {
            image::Rgba([255, 0, 0, 255])
        }
    });
    let result = decode(&png(image)).unwrap();
    assert_eq!(result.pixels.len(), 65536);
    let center = ((EDGE / 2 * EDGE + EDGE / 2) * 4) as usize;
    assert_eq!(&result.pixels[center..center + 4], &[0, 0, 255, 255]);
    assert!(decode(b"not an image").is_none());
    assert!(decode(&vec![0; MAX_DOWNLOAD + 1]).is_none());
    assert!(decode(&png(image::RgbaImage::new(2049, 1))).is_none());
    assert!(Thumbnail::from_pixels(vec![0; 20]).is_none());
    let mut jpeg = Cursor::new(Vec::new());
    image::RgbImage::from_pixel(48, 48, image::Rgb([64, 128, 192]))
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    assert!(decode(jpeg.get_ref()).is_some());
}

type Reply = oneshot::Sender<Result<Outcome, FetchError>>;
struct Controlled(mpsc::UnboundedSender<(String, Reply)>);
#[async_trait::async_trait]
impl Source for Controlled {
    async fn fetch(&self, entry: &AvatarEntry) -> Result<Outcome, FetchError> {
        let (tx, rx) = oneshot::channel();
        self.0
            .send((entry.jid.clone(), tx))
            .map_err(|_| FetchError::Retry)?;
        rx.await.map_err(|_| FetchError::Retry)?
    }
}
async fn called(rx: &mut mpsc::UnboundedReceiver<(String, Reply)>) -> (String, Reply) {
    timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn worker_is_lazy_reuses_positive_and_negative_results_and_stops_in_flight() {
    let (_temp, db) = fixture();
    db.bind_account(PN.into(), "Synthetic owner".into())
        .await
        .unwrap();
    let own = db.create_chat(PN.into()).await.unwrap();
    let other = db.create_chat("555@g.us".into()).await.unwrap();
    let (demand, rx) = watch::channel(vec![0.into(), own.into(), other.into()]);
    let (online, on) = watch::channel(false);
    let (stop, stopped) = watch::channel(false);
    let (calls, mut requests) = mpsc::unbounded_channel();
    let task = tokio::spawn(fetch_worker(db.clone(), Controlled(calls), rx, on, stopped));
    assert!(
        timeout(Duration::from_millis(50), requests.recv())
            .await
            .is_err()
    );
    online.send_replace(true);
    let (jid, reply) = called(&mut requests).await;
    assert_eq!(jid, PN);
    assert!(reply.send(Ok(picture(9))).is_ok());
    let (jid, reply) = called(&mut requests).await;
    assert_eq!(jid, "555@g.us", "self aliases must share one fetch");
    assert!(reply.send(Ok(Outcome::Absent)).is_ok());
    // Updating demand to the same chats must not retry positive or absent photos.
    demand.send_replace(vec![other.into(), own.into(), 0.into()]);
    assert!(
        timeout(Duration::from_millis(250), requests.recv())
            .await
            .is_err()
    );
    let entries = db
        .avatar_entries(vec![0.into(), own.into(), other.into()], true)
        .await
        .unwrap();
    assert!(entries[0].has_photo && entries[1].has_photo);
    assert!(!entries[2].has_photo && entries[2].retry_at > chrono::Utc::now().timestamp());
    db.invalidate_avatar(PN.into(), false, 100).await.unwrap();
    let (_, pending) = called(&mut requests).await;
    stop.send_replace(true);
    timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        pending.send(Ok(picture(12))).is_err(),
        "cancelled fetch cannot commit"
    );
}

#[tokio::test]
async fn rate_limit_stops_the_whole_queue_and_transient_failure_keeps_cached_photo() {
    let (_temp, db) = fixture();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let other = db.create_chat("555@g.us".into()).await.unwrap();
    let waiting = db.create_chat("556@g.us".into()).await.unwrap();
    let cached = db
        .avatar_entries(vec![chat.into()], false)
        .await
        .unwrap()
        .remove(0);
    db.save_avatar(cached, picture(15), 0).await.unwrap();
    let (_demand, rx) = watch::channel(vec![chat.into(), other.into(), waiting.into()]);
    let (_online, on) = watch::channel(true);
    let (stop, stopped) = watch::channel(false);
    let (calls, mut requests) = mpsc::unbounded_channel();
    let task = tokio::spawn(fetch_worker(db.clone(), Controlled(calls), rx, on, stopped));
    let (_, reply) = called(&mut requests).await;
    assert!(reply.send(Err(FetchError::Retry)).is_ok());
    let (_, reply) = called(&mut requests).await;
    assert!(
        reply
            .send(Err(FetchError::RateLimited(Duration::from_secs(120))))
            .is_ok()
    );
    assert!(
        timeout(Duration::from_millis(250), requests.recv())
            .await
            .is_err()
    );
    let entries = db
        .avatar_entries(vec![chat.into(), other.into()], true)
        .await
        .unwrap();
    assert_eq!(entries[0].photo.as_ref().unwrap().pixels[0], 15);
    assert!(entries[1].retry_at > chrono::Utc::now().timestamp() + 90);
    stop.send_replace(true);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn offline_snapshots_reuse_pixels_remove_private_photos_and_bound_residency() {
    let (_temp, db) = fixture();
    let first = db.create_chat(PN.into()).await.unwrap();
    let second = db.create_chat("555@g.us".into()).await.unwrap();
    for id in [first, second] {
        let entry = db
            .avatar_entries(vec![id.into()], false)
            .await
            .unwrap()
            .remove(0);
        db.save_avatar(entry, picture(id as u8), 0).await.unwrap();
    }
    let (archives, rx) = watch::channel(None);
    let (demand, requests) = watch::channel(vec![first.into()]);
    let (updates, mut snapshots) = watch::channel(Snapshot::default());
    let task = tokio::spawn(present(rx, requests, updates));
    archives.send_replace(Some(db.clone()));
    timeout(Duration::from_secs(2), snapshots.changed())
        .await
        .unwrap()
        .unwrap();
    let initial = snapshots.borrow_and_update().clone();
    assert_eq!(initial.len(), 1);
    db.changed();
    assert!(
        timeout(Duration::from_millis(150), snapshots.changed())
            .await
            .is_err()
    );
    demand.send_replace(vec![first.into(), second.into()]);
    timeout(Duration::from_secs(2), snapshots.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(
        &initial[&first.into()],
        &snapshots.borrow()[&first.into()]
    ));
    demand.send_replace(vec![second.into()]);
    timeout(Duration::from_secs(2), snapshots.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshots.borrow().len(), 1);
    assert!(!snapshots.borrow().contains_key(&first.into()));
    db.invalidate_avatar("555@g.us".into(), true, 100)
        .await
        .unwrap();
    timeout(Duration::from_secs(2), snapshots.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(snapshots.borrow().is_empty());
    drop(demand);
    timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
}
