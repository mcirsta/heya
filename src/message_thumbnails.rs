//! On-demand cached preview images. No website requests and no UI-thread decode.
use crate::{archive::Archive, photos::Pixels};
use image::ImageDecoder;
use rusqlite::OptionalExtension;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::watch;

pub const MAX_DEMAND: usize = 32;
pub const MAX_ENCODED: usize = 256 * 1024;
pub const EDGE: u32 = 480;
pub type Snapshot = Arc<BTreeMap<i32, Arc<Pixels>>>;

/// Bounded worker-only decoding. Preserve the whole thumbnail so document
/// strips can show its top edge; link cards still crop to their square in Slint.
pub fn decode(bytes: &[u8]) -> Option<Pixels> {
    if bytes.is_empty() || bytes.len() > MAX_ENCODED {
        return None;
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
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
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut image = image::DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    let edge = EDGE.min(image.width().max(image.height()));
    let image = image.thumbnail(edge, edge).into_rgba8();
    Some(Pixels {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

pub async fn demo_present(mut demand: watch::Receiver<Vec<i32>>, updates: watch::Sender<Snapshot>) {
    let Ok((link, document, video)) = tokio::task::spawn_blocking(|| {
        let link = crate::fixtures::message_image_samples([0])[&0].clone();
        let document = crate::fixtures::document_thumbnail();
        let document = image::imageops::crop_imm(
            &document,
            0,
            0,
            crate::document_previews::WIDTH,
            crate::document_previews::HEIGHT,
        )
        .to_image();
        (
            link,
            Arc::new(Pixels {
                width: document.width(),
                height: document.height(),
                rgba: document.into_raw(),
            }),
            Arc::new(crate::fixtures::video_pixels(0.)),
        )
    })
    .await
    else {
        return;
    };
    loop {
        let snapshot = demand
            .borrow_and_update()
            .iter()
            .take(MAX_DEMAND)
            .map(|id| {
                (
                    *id,
                    if (24000..25000).contains(id) {
                        document.clone()
                    } else if *id >= 27000 {
                        video.clone()
                    } else {
                        link.clone()
                    },
                )
            })
            .collect();
        updates.send_replace(Arc::new(snapshot));
        if demand.changed().await.is_err() {
            break;
        }
    }
}

/// Positive and negative results last only for the current visible demand.
/// Cache revisions replace embedded document thumbnails once, without rereading
/// or decoding unchanged pixels on receipt/metadata updates.
pub async fn present(
    mut archives: watch::Receiver<Option<Archive>>,
    mut demand: watch::Receiver<Vec<i32>>,
    updates: watch::Sender<Snapshot>,
) {
    let mut archive = archives.borrow_and_update().clone();
    let mut changes = archive.as_ref().map(Archive::subscribe);
    let mut first = true;
    let mut known: BTreeMap<i32, (i64, Option<Arc<Pixels>>)> = BTreeMap::new();
    let mut archives_open = true;
    loop {
        if !first {
            tokio::select! {
                result = archives.changed(), if archives_open => {
                    if result.is_err() { archives_open=false; continue; }
                    archive=archives.borrow_and_update().clone();
                    changes=archive.as_ref().map(Archive::subscribe);
                    known.clear();
                    if archive.is_none() { updates.send_replace(Default::default()); }
                }
                result = demand.changed() => { if result.is_err() { return; } }
                _ = async { match &mut changes { Some(c) => { let _ = c.changed().await; }, None => std::future::pending().await } } => {}
            }
        }
        first = false;
        let Some(db) = &archive else {
            continue;
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        if let Some(changes) = &mut changes {
            changes.borrow_and_update();
        }
        let ids: BTreeSet<_> = demand
            .borrow_and_update()
            .iter()
            .copied()
            .take(MAX_DEMAND)
            .collect();
        if ids.is_empty() {
            known.clear();
            updates.send_if_modified(|old| {
                if old.is_empty() {
                    return false;
                }
                *old = Default::default();
                true
            });
            continue;
        }
        let cached: BTreeMap<_, _> = known
            .iter()
            .map(|(id, (revision, _))| (*id, *revision))
            .collect();
        let Ok(entries) = db
            .call(move |db| {
                let mut query = db.prepare_cached(
                    "SELECT revision, CASE WHEN revision=?2 THEN NULL ELSE bytes END, document FROM (
                        SELECT CASE WHEN length(p.preview)>0 THEN p.id ELSE 0 END revision,
                        CASE WHEN length(p.preview)>0 THEN p.preview ELSE m.thumbnail END bytes,
                        json_type(nullif(m.content,''),'$.document')='object' document
                        FROM messages m LEFT JOIN document_previews p ON p.message_id=m.id
                        WHERE m.id=?1 AND m.visible=1 AND m.deleted=0
                        AND (m.expires IS NULL OR m.expires>unixepoch())
                    ) WHERE length(bytes)>0 AND length(bytes)<=?3",
                )?;
                let mut entries = Vec::new();
                for id in ids {
                    let entry = query.query_row(
                        rusqlite::params![id, cached.get(&id).copied().unwrap_or(-1), crate::document_previews::MAX_BYTES as i64],
                        |r| Ok((r.get::<_,i64>(0)?,r.get::<_,Option<Vec<u8>>>(1)?,r.get::<_,Option<bool>>(2)?.unwrap_or(false))),
                    ).optional()?;
                    if let Some((revision,bytes,document)) = entry {
                        entries.push((id,revision,bytes,document));
                    }
                }
                Ok(entries)
            })
            .await
        else {
            continue;
        };
        let decoded = if entries.iter().all(|(_, _, bytes, _)| bytes.is_none()) {
            // Receipt/metadata changes only revalidate existence, without a
            // decode job, fresh pixel allocation or renderer image update.
            entries
                .into_iter()
                .map(|(id, revision, _, _)| (id, revision, None))
                .collect()
        } else {
            let Ok(decoded) = tokio::task::spawn_blocking(move || {
                entries
                    .into_iter()
                    .map(|(id, revision, bytes, document)| {
                        (
                            id,
                            revision,
                            bytes.map(|b| {
                                if document {
                                    crate::document_previews::decode(&b)
                                } else {
                                    decode(&b)
                                }
                                .map(Arc::new)
                            }),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .await
            else {
                continue;
            };
            decoded
        };
        let mut next = BTreeMap::new();
        for (id, revision, photo) in decoded {
            next.insert(
                id,
                (
                    revision,
                    photo.unwrap_or_else(|| known.get(&id).and_then(|(_, p)| p.clone())),
                ),
            );
        }
        known = next;
        let snapshot: BTreeMap<_, _> = known
            .iter()
            .filter_map(|(id, (_, p))| p.clone().map(|p| (*id, p)))
            .collect();
        updates.send_if_modified(|old| {
            if old.len() == snapshot.len()
                && snapshot
                    .iter()
                    .all(|(id, p)| old.get(id).is_some_and(|old| Arc::ptr_eq(old, p)))
            {
                return false;
            }
            *old = Arc::new(snapshot);
            true
        });
    }
}
