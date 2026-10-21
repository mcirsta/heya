//! Bounded received-photo work. No toolkit or protocol types cross this layer.
use crate::archive::{self, Archive};
use image::{ImageDecoder, ImageEncoder};
#[cfg(test)]
use std::time::Duration;
use std::{collections::BTreeMap, io::Cursor, sync::Arc};
use tokio::sync::watch;

pub const MAX_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_DEMAND: usize = 24;
pub const INLINE_EDGE: u32 = 640;
pub const VIEWER_EDGE: u32 = 2048;
pub const THUMB_EDGE: u32 = 96;
pub const THUMB_COUNT: usize = 7;

/// A viewer selection resolved from the archive, independent of timeline paging.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Gallery {
    pub id: i32,
    pub members: Vec<i32>,
    pub position: usize,
    pub sender: String,
    pub date: String,
    pub caption: String,
}
impl Gallery {
    pub fn thumbnails(&self) -> &[i32] {
        let start = self
            .position
            .saturating_sub(THUMB_COUNT / 2)
            .min(self.members.len().saturating_sub(THUMB_COUNT));
        &self.members[start..(start + THUMB_COUNT).min(self.members.len())]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum State {
    #[default]
    Idle = 0,
    Loading = 1,
    Ready = 2,
    Failed = 3,
    TooLarge = 4,
    Unavailable = 5,
    Invalid = 6,
}
impl State {
    pub fn from_db(value: i32) -> Self {
        match value {
            1 => Self::Loading,
            2 => Self::Ready,
            3 => Self::Failed,
            4 => Self::TooLarge,
            5 => Self::Unavailable,
            6 => Self::Invalid,
            _ => Self::Idle,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Download photo",
            Self::Loading => "Downloading…",
            Self::Ready => "Open photo",
            Self::Failed => "Download failed · click to retry",
            Self::TooLarge => "Photo exceeds the 20 MiB limit",
            Self::Unavailable => "Photo unavailable · try again or ask the sender",
            Self::Invalid => "Could not display photo · click to retry",
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Demand {
    pub chat: i32,
    pub visible: Vec<i32>,
    pub viewer: Option<i32>,
    pub automatic: bool,
    // A monotonic serial makes two explicit retries distinguishable.
    pub retry: Option<(i32, u64)>,
}
impl Demand {
    pub fn ids(&self) -> Vec<i32> {
        let mut ids = Vec::with_capacity(MAX_DEMAND + 1);
        if let Some(id) = self.viewer {
            ids.push(id);
        }
        for &id in self.visible.iter().take(MAX_DEMAND) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }
}

pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
#[derive(Clone)]
pub struct Photo {
    pub state: State,
    pub image: Option<Arc<Pixels>>,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub rows: BTreeMap<i32, Photo>,
    pub viewer: Option<(i32, Arc<Pixels>)>,
    pub checked: Option<(i32, i32)>,
    pub gallery: Option<Arc<Gallery>>,
    pub thumbnails: BTreeMap<i32, Thumbnail>,
}
#[derive(Clone)]
pub struct Thumbnail {
    pub pixels: Arc<Pixels>,
    pub key: u64,
}
impl Thumbnail {
    fn new(pixels: Pixels) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            pixels: Arc::new(pixels),
            key: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }
}
pub struct Request {
    pub id: i32,
    pub raw: Vec<u8>,
}
pub struct Cached {
    pub original: Vec<u8>,
    pub preview: Vec<u8>,
}
#[async_trait::async_trait]
pub trait Source: Send + Sync {
    async fn fetch(&self, raw: &[u8]) -> Result<Vec<u8>, State>;
}

fn decoded(bytes: &[u8]) -> Option<image::DynamicImage> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
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
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 32_000_000 {
        return None;
    }
    let orientation = decoder.orientation().ok()?;
    let mut image = image::DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    Some(image)
}

pub fn decode(bytes: &[u8], edge: u32) -> Option<Pixels> {
    decode_with_dimensions(bytes, edge).map(|(_, pixels)| pixels)
}
pub(crate) fn decode_with_dimensions(bytes: &[u8], edge: u32) -> Option<((u32, u32), Pixels)> {
    let image = decoded(bytes)?;
    let dimensions = (image.width(), image.height());
    let image = resized(image, edge);
    Some((
        dimensions,
        Pixels {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        },
    ))
}
fn resized(image: image::DynamicImage, edge: u32) -> image::RgbaImage {
    // DynamicImage::thumbnail also upscales. Preserve small embedded previews
    // and originals instead of allocating a full-size buffer for tiny images.
    let edge = edge.min(image.width().max(image.height()));
    image.thumbnail(edge, edge).into_rgba8()
}
fn prepare(original: Vec<u8>) -> Option<Cached> {
    let image = resized(decoded(&original)?, INLINE_EDGE);
    let mut preview = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut preview,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )
    .ok()?;
    Some(Cached { original, preview })
}

// Cancelled blocking decodes retain their permit until they really finish.
static DECODE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let permit = DECODE.acquire().await.ok()?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f()
    })
    .await
    .ok()
}

/// One download, no eager history sweep, and no automatic retry loop on failures.
pub async fn fetch_worker(
    archive: Archive,
    source: impl Source,
    mut demand: watch::Receiver<Demand>,
    mut online: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
) -> archive::Result<()> {
    archive.reset_photo_downloads().await?;
    let mut changed = archive.subscribe();
    let mut attempted_retry = None;
    loop {
        if *stop.borrow() || stop.has_changed().is_err() || demand.has_changed().is_err() {
            return Ok(());
        }
        let wanted = demand.borrow_and_update().clone();
        changed.borrow_and_update();
        if *online.borrow() {
            let retry = wanted.retry.filter(|r| Some(*r) != attempted_retry);
            let rows = archive.photo_candidates(wanted.ids()).await?;
            if let Some(&(id, _)) = rows.iter().find(|&&(id, state)| {
                retry.is_some_and(|r| r.0 == id)
                    || ((wanted.automatic || wanted.viewer == Some(id)) && state == State::Idle)
            }) {
                if retry.is_some_and(|r| r.0 == id) {
                    attempted_retry = retry;
                }
                let Some(request) = archive.photo_request(id).await? else {
                    continue;
                };
                archive.photo_state(&request, State::Loading).await?;
                let work = async {
                    let bytes = source.fetch(&request.raw).await?;
                    blocking(move || prepare(bytes))
                        .await
                        .flatten()
                        .ok_or(State::Invalid)
                };
                tokio::pin!(work);
                let result = loop {
                    tokio::select! {
                        biased;
                        _ = stop.changed() => { break None; }
                        _ = online.changed() => { if !*online.borrow() { break None; } }
                        result = demand.changed() => {
                            if result.is_err() { break None; }
                            let next = demand.borrow_and_update().clone();
                            if !next.ids().contains(&id)
                                || (next.viewer != wanted.viewer && next.viewer.is_some_and(|viewer| viewer != id))
                            { break None; }
                            // Repeated clicks while this attempt is in flight
                            // belong to the same attempt, not a second download.
                            if let Some(retry) = next.retry.filter(|r| r.0 == id) {
                                attempted_retry = Some(retry);
                            }
                        }
                        result = changed.changed() => {
                            if result.is_err() || archive.photo_request(id).await?.is_none_or(|current|current.raw!=request.raw) { break None; }
                        }
                        result = &mut work => { break Some(result); }
                    }
                };
                match result {
                    Some(Ok(photo)) => {
                        archive.save_photo(&request, photo).await?;
                    }
                    Some(Err(state)) => {
                        archive.photo_state(&request, state).await?;
                    }
                    None => {
                        archive.photo_state(&request, State::Idle).await?;
                    }
                }
                continue;
            }
        }
        tokio::select! {
            _ = stop.changed() => {},
            _ = online.changed() => {},
            _ = demand.changed() => {},
            _ = changed.changed() => {},
        }
    }
}

#[path = "photos_present.rs"]
mod presentation;
pub use presentation::present;
#[cfg(test)]
#[path = "photo_tests.rs"]
mod tests;

/// Offline fixture pump: identical demand/publication boundary, synthetic pixels.
pub async fn demo_present(
    mut demand: watch::Receiver<Demand>,
    updates: watch::Sender<Arc<Snapshot>>,
) {
    let mut samples = Vec::new();
    let mut viewer: Option<(i32, Arc<Pixels>)> = None;
    loop {
        let wanted = demand.borrow_and_update().clone();
        if samples.is_empty() && !wanted.ids().is_empty() {
            samples = blocking(|| {
                (0..3)
                    .map(|i| Arc::new(crate::fixtures::photo_pixels(i, INLINE_EDGE)))
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            if samples.len() != 3 {
                return;
            }
        }
        let mut result = Snapshot::default();
        for id in wanted.ids() {
            result.rows.insert(
                id,
                Photo {
                    state: State::Ready,
                    image: Some(samples[id.unsigned_abs() as usize % 3].clone()),
                },
            );
        }
        if wanted.viewer != viewer.as_ref().map(|v| v.0) {
            viewer = if let Some(id) = wanted.viewer {
                blocking(move || {
                    crate::fixtures::photo_pixels(id.unsigned_abs() as usize, VIEWER_EDGE)
                })
                .await
                .map(|p| (id, Arc::new(p)))
            } else {
                None
            };
        }
        result.viewer = viewer.clone();
        if let Some(id) = wanted.viewer {
            let gallery = demo_gallery(wanted.chat, id);
            result.checked = Some((wanted.chat, id));
            if gallery.members.len() > 1 {
                for &id in gallery.thumbnails() {
                    if let Some(thumbnail) = blocking(move || {
                        Thumbnail::new(crate::fixtures::photo_pixels(
                            id.unsigned_abs() as usize,
                            THUMB_EDGE,
                        ))
                    })
                    .await
                    {
                        result.thumbnails.insert(id, thumbnail);
                    }
                }
            }
            result.gallery = Some(Arc::new(gallery));
        }
        updates.send_replace(Arc::new(result));
        if demand.changed().await.is_err() {
            return;
        }
    }
}

/// Only the explicit synthetic photo fixture forms a demo album. Other photos
/// remain standalone; production associations always come from the protocol.
fn demo_gallery(chat: i32, id: i32) -> Gallery {
    let members = if chat == 9 && (22002..=22004).contains(&id) {
        (22002..=22004).collect::<Vec<_>>()
    } else {
        vec![id]
    };
    let position = members.iter().position(|&member| member == id).unwrap_or(0);
    Gallery {
        id,
        members,
        position,
        sender: "Mira Sample".into(),
        date: "Today, 10:24".into(),
        caption: match id {
            22002 => "A synthetic landscape · Salut 👋 · こんにちは",
            22004 => "A wide photo keeps its proportions.",
            _ => "",
        }
        .into(),
    }
}
