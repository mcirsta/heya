//! Bounded thumbnail work, independent of Slint and the protocol SDK.
use crate::archive::{self, Archive, AvatarEntry};
use std::{collections::BTreeMap, io::Cursor, sync::Arc, time::Duration};
use tokio::sync::watch;

pub const EDGE: u32 = 128;
pub const MAX_DEMAND: usize = 66; // account, selected chat, visible senders/sidebar
pub const MAX_DOWNLOAD: usize = 2 * 1024 * 1024;

pub struct Thumbnail {
    pub pixels: Arc<[u8]>,
}
impl Thumbnail {
    pub fn from_pixels(pixels: Vec<u8>) -> Option<Self> {
        (pixels.len() == (EDGE * EDGE * 4) as usize).then(|| Self {
            pixels: pixels.into(),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Chat(i32),
    Participant(String),
}
impl From<i32> for Target {
    fn from(chat: i32) -> Self {
        Self::Chat(chat)
    }
}
pub type Snapshot = Arc<BTreeMap<Target, Arc<Thumbnail>>>;

pub enum Outcome {
    Photo { id: String, photo: Thumbnail },
    Absent,
    Unchanged,
}
pub enum FetchError {
    Retry,
    RateLimited(Duration),
}
#[async_trait::async_trait]
pub trait Source: Send + Sync {
    async fn fetch(&self, entry: &AvatarEntry) -> Result<Outcome, FetchError>;
}

/// Invoke on a blocking worker. Bound encoded input, dimensions and allocation
/// before decoding. Crop centrally once; all UI sizes share the same thumbnail.
pub fn decode(bytes: &[u8]) -> Option<Thumbnail> {
    if bytes.len() > MAX_DOWNLOAD {
        return None;
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    if !matches!(
        reader.format(),
        Some(image::ImageFormat::Jpeg | image::ImageFormat::Png)
    ) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    Thumbnail::from_pixels(
        image
            .resize_to_fill(EDGE, EDGE, image::imageops::FilterType::Triangle)
            .to_rgba8()
            .into_raw(),
    )
}

/// One coalesced demand set, one in-flight fetch, no eager address-book sweep.
/// The supervisor owns this worker and joins it before releasing the SDK store.
pub async fn fetch_worker(
    archive: Archive,
    source: impl Source,
    mut demand: watch::Receiver<Vec<Target>>,
    mut online: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
) -> archive::Result<()> {
    let mut changed = archive.subscribe_avatars();
    let mut chats = archive.subscribe();
    let mut retry = tokio::time::interval(Duration::from_secs(30));
    let mut global_backoff = tokio::time::Instant::now();
    loop {
        if *stop.borrow() || stop.has_changed().is_err() || demand.has_changed().is_err() {
            return Ok(());
        }
        if *online.borrow() && tokio::time::Instant::now() >= global_backoff {
            let ids = demand.borrow_and_update().clone();
            let entries = archive.avatar_entries(ids, false).await?;
            let now = chrono::Utc::now().timestamp();
            if let Some(entry) = entries.into_iter().find(|e| e.retry_at <= now) {
                let fetched = tokio::select! {
                    biased;
                    _ = stop.changed() => continue,
                    _ = online.changed() => continue,
                    result = source.fetch(&entry) => result,
                };
                let (outcome, next) = match fetched {
                    Ok(outcome) => (outcome, now + 86400),
                    Err(FetchError::Retry) => (Outcome::Unchanged, now + 300),
                    Err(FetchError::RateLimited(delay)) => {
                        let delay = delay.max(Duration::from_secs(60));
                        global_backoff = tokio::time::Instant::now() + delay;
                        (
                            Outcome::Unchanged,
                            now + delay.as_secs().min(i64::MAX as u64) as i64,
                        )
                    }
                };
                archive.save_avatar(entry, outcome, next).await?;
                tokio::select! {
                    _ = stop.changed() => {},
                    _ = online.changed() => {},
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {},
                }
                continue;
            }
        }
        tokio::select! {
            _ = stop.changed() => {},
            _ = online.changed() => {},
            result = demand.changed() => {if result.is_err() {return Ok(());}},
            _ = changed.changed() => {},
            _ = chats.changed() => {},
            _ = retry.tick() => {},
        }
    }
}

/// Cached photos work offline. Each snapshot holds only the current demand set;
/// unchanged images retain identity so Slint does not upload textures repeatedly.
pub async fn present(
    mut archives: watch::Receiver<Option<Archive>>,
    mut demand: watch::Receiver<Vec<Target>>,
    updates: watch::Sender<Snapshot>,
) {
    let mut archive: Option<Archive> = None;
    let mut photos = None;
    let mut chats = None;
    let mut known: BTreeMap<Target, (String, i64, Arc<Thumbnail>)> = BTreeMap::new();
    let mut archives_open = true;
    loop {
        tokio::select! {
            result = archives.changed(), if archives_open => {
                if result.is_err() {archives_open=false;continue;}
                archive=archives.borrow_and_update().clone();
                photos=archive.as_ref().map(Archive::subscribe_avatars);
                chats=archive.as_ref().map(Archive::subscribe);
                known.clear();
            }
            result = demand.changed() => {if result.is_err() {return;}},
            _ = changed(&mut photos) => {},
            _ = changed(&mut chats) => {},
        }
        let Some(db) = &archive else {
            continue;
        };
        // Coalesce history bursts before querying; only new/changed thumbnails
        // read pixels. A receipt must not reload every visible image blob.
        tokio::time::sleep(Duration::from_millis(40)).await;
        if let Some(changed) = &mut chats {
            changed.borrow_and_update();
        }
        if let Some(changed) = &mut photos {
            changed.borrow_and_update();
        }
        let ids = demand.borrow_and_update().clone();
        let Ok(entries) = db.avatar_entries(ids, false).await else {
            continue;
        };
        let needed = entries
            .iter()
            .filter(|entry| {
                entry.has_photo
                    && !known
                        .get(&entry.target)
                        .is_some_and(|(jid, generation, _)| {
                            *jid == entry.jid && *generation == entry.generation
                        })
            })
            .map(|entry| entry.target.clone())
            .collect();
        let Ok(loaded) = db.avatar_entries(needed, true).await else {
            continue;
        };
        let mut loaded: BTreeMap<_, _> = loaded
            .into_iter()
            .map(|entry| (entry.target.clone(), entry))
            .collect();
        let mut next = BTreeMap::new();
        let mut snapshot = BTreeMap::new();
        for entry in entries {
            if !entry.has_photo {
                continue;
            }
            let photo = known
                .get(&entry.target)
                .filter(|(jid, generation, _)| *jid == entry.jid && *generation == entry.generation)
                .map(|(_, _, photo)| photo.clone())
                .or_else(|| {
                    loaded
                        .remove(&entry.target)
                        .filter(|loaded| {
                            loaded.generation == entry.generation && loaded.jid == entry.jid
                        })
                        .and_then(|entry| entry.photo)
                });
            let Some(photo) = photo else {
                continue;
            };
            snapshot.insert(entry.target.clone(), photo.clone());
            next.insert(entry.target, (entry.jid, entry.generation, photo));
        }
        known = next;
        updates.send_if_modified(|previous| {
            if previous.len() == snapshot.len()
                && snapshot
                    .iter()
                    .all(|(id, p)| previous.get(id).is_some_and(|old| Arc::ptr_eq(old, p)))
            {
                return false;
            }
            *previous = Arc::new(snapshot);
            true
        });
    }
}

async fn changed(receiver: &mut Option<watch::Receiver<u64>>) {
    match receiver {
        Some(receiver) => {
            let _ = receiver.changed().await;
        }
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "avatar_tests.rs"]
mod tests;
