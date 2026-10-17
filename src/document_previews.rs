//! Demand-driven document preview upgrades. The source supplies an image from a
//! remote thumbnail or isolated first-page rendering. No toolkit or SDK types
//! cross this API; only the bounded derived image is retained.
use crate::{
    archive::{self, Archive},
    message_thumbnails::MAX_DEMAND,
    photos::Pixels,
};
use image::{ImageDecoder, ImageEncoder};
use std::{io::Cursor, time::Duration};
use tokio::sync::watch;

pub const MAX_BYTES: usize = 1024 * 1024;
pub const WIDTH: u32 = 768;
pub const HEIGHT: u32 = 320;

pub fn sharp_enough(bytes: &[u8]) -> bool {
    if bytes.len() > MAX_BYTES {
        return false;
    }
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .is_some_and(|(w, h)| (WIDTH..=4096).contains(&w) && (HEIGHT..=4096).contains(&h))
}

pub struct Request {
    pub id: i32,
    pub raw: Vec<u8>,
}
#[async_trait::async_trait]
pub trait Source: Send + Sync {
    async fn fetch(&self, raw: &[u8]) -> Option<Vec<u8>>;
}

/// Retain enough pixels for a 340 logical-pixel strip on HiDPI displays. Crop
/// before resizing so a portrait page does not waste memory below the strip.
/// JPEG/PNG only, with bounded input, decode and output allocations.
pub fn decode(bytes: &[u8]) -> Option<Pixels> {
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
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut page = image::DynamicImage::from_decoder(decoder).ok()?;
    page.apply_orientation(orientation);
    if page.width() == 0 || page.height() == 0 {
        return None;
    }
    let height = page.height().min((page.width() * HEIGHT / WIDTH).max(1));
    let strip = page.crop_imm(0, 0, page.width(), height);
    let width = strip.width().min(WIDTH);
    let height = strip.height().min(HEIGHT);
    let strip = strip.thumbnail(width, height).into_rgba8();
    Some(Pixels {
        width: strip.width(),
        height: strip.height(),
        rgba: strip.into_raw(),
    })
}
fn prepare(bytes: Vec<u8>) -> Option<Vec<u8>> {
    let pixels = decode(&bytes)?;
    let mut preview = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut preview,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        &pixels.rgba,
        pixels.width,
        pixels.height,
        image::ExtendedColorType::Rgba8,
    )
    .ok()?;
    (preview.len() <= MAX_BYTES).then_some(preview)
}

/// Serial, cancellable work for nearby documents. Failed references get a
/// persisted cooldown; reopening a chat or reconnecting cannot spin retries.
pub async fn fetch_worker(
    archive: Archive,
    source: impl Source,
    mut demand: watch::Receiver<Vec<i32>>,
    mut online: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
) -> archive::Result<()> {
    let mut changed = archive.subscribe();
    loop {
        if *stop.borrow()
            || stop.has_changed().is_err()
            || demand.has_changed().is_err()
            || online.has_changed().is_err()
        {
            return Ok(());
        }
        let ids = demand
            .borrow_and_update()
            .iter()
            .copied()
            .take(MAX_DEMAND)
            .collect();
        changed.borrow_and_update();
        if *online.borrow_and_update() {
            // Let a burst of wheel events settle before reading saved envelopes.
            tokio::time::sleep(Duration::from_millis(40)).await;
            if demand.has_changed().unwrap_or(true) || !*online.borrow() || *stop.borrow() {
                continue;
            }
            if let Some(request) = archive.document_preview_request(ids).await? {
                let work = async {
                    let bytes = source.fetch(&request.raw).await?;
                    crate::photos::blocking(move || prepare(bytes))
                        .await
                        .flatten()
                };
                tokio::pin!(work);
                let result = loop {
                    tokio::select! {
                        biased;
                        _ = stop.changed() => { break None; }
                        _ = online.changed() => { if !*online.borrow() { break None; } }
                        result = demand.changed() => {
                            if result.is_err() || !demand.borrow_and_update().iter().take(MAX_DEMAND).any(|id|*id==request.id) { break None; }
                        }
                        _ = changed.changed() => {
                            if !archive.document_preview_current(&request).await? { break None; }
                        }
                        result = &mut work => { break Some(result); }
                    }
                };
                if let Some(preview) = result {
                    archive.save_document_preview(&request, preview).await?;
                }
                continue;
            }
        }
        tokio::select! {
            _ = stop.changed() => {}, _ = online.changed() => {},
            _ = demand.changed() => {}, _ = changed.changed() => {},
        }
    }
}
#[cfg(test)]
#[path = "document_preview_tests.rs"]
mod tests;
