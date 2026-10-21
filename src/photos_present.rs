//! Decode only demanded photos and the single open viewer, with immutable reuse.
use super::*;

pub async fn present(
    archives: watch::Receiver<Option<Archive>>,
    demand: watch::Receiver<Demand>,
    updates: watch::Sender<Arc<Snapshot>>,
) {
    present_with_decode(archives, demand, updates, |bytes, edge| async move {
        blocking(move || decode(&bytes, edge)).await.flatten()
    })
    .await;
}

// A decoder seam lets tests hold a slow photo without relying on CPU timing.
pub(super) async fn present_with_decode<F, Fut>(
    mut archives: watch::Receiver<Option<Archive>>,
    mut demand: watch::Receiver<Demand>,
    updates: watch::Sender<Arc<Snapshot>>,
    decode: F,
) where
    F: Fn(Vec<u8>, u32) -> Fut,
    Fut: std::future::Future<Output = Option<Pixels>>,
{
    let mut archive = archives.borrow_and_update().clone();
    let mut changes = archive.as_ref().map(Archive::subscribe);
    let mut known: BTreeMap<i32, (i64, Option<Arc<Pixels>>)> = BTreeMap::new();
    let mut viewer: Option<(i32, i64, Option<Arc<Pixels>>)> = None;
    let mut thumbs: BTreeMap<i32, (i64, Option<Thumbnail>)> = BTreeMap::new();
    let mut first = true;
    let mut archives_open = true;
    loop {
        if !first {
            tokio::select! {
                result = archives.changed(), if archives_open => {
                    if result.is_err() { archives_open=false; continue; }
                    archive=archives.borrow_and_update().clone();
                    changes=archive.as_ref().map(Archive::subscribe);
                    known.clear(); viewer=None; thumbs.clear();
                    updates.send_replace(Default::default());
                }
                result = demand.changed() => { if result.is_err() { return; } }
                _ = async { match &mut changes {Some(c)=>{let _=c.changed().await;},None=>std::future::pending().await} } => {}
            }
        }
        first = false;
        let Some(db) = &archive else { continue };
        // watch already coalesces demand while work is in flight. A timed
        // debounce here made newly exposed photos wait on every wheel step.
        let wanted = demand.borrow_and_update().clone();
        if let Some(changes) = &mut changes {
            changes.borrow_and_update();
        }
        let ids = wanted.ids();
        let gallery = if let Some(id) = wanted.viewer {
            match db.photo_gallery(wanted.chat, id).await {
                Ok(g) => g.map(Arc::new),
                Err(_) => continue,
            }
        } else {
            None
        };
        let thumb_ids = gallery
            .as_ref()
            .filter(|g| g.members.len() > 1)
            .map(|g| g.thumbnails().to_vec())
            .unwrap_or_default();
        thumbs.retain(|id, _| thumb_ids.contains(id));
        known.retain(|id, _| ids.contains(id));
        if viewer.as_ref().map(|v| v.0) != wanted.viewer {
            viewer = None;
        }
        let keys = known.iter().map(|(id, (key, _))| (*id, *key)).collect();
        let viewer_key = wanted.viewer.map(|id| (id, viewer.as_ref().map(|v| v.1)));
        let Ok(rows) = db.photo_rows(ids, keys, viewer_key).await else {
            continue;
        };
        known.retain(|id, _| rows.iter().any(|r| r.id == *id));
        if wanted
            .viewer
            .is_some_and(|id| !rows.iter().any(|r| r.id == id))
        {
            viewer = None;
        }

        // Reuse all unchanged visible images before decoding anything new. The
        // renderer receives the same pixel identity, so no texture re-upload.
        let mut result = Snapshot {
            checked: wanted.viewer.map(|id| (wanted.chat, id)),
            gallery,
            thumbnails: thumbs
                .iter()
                .filter_map(|(id, (_, p))| p.as_ref().map(|p| (*id, p.clone())))
                .collect(),
            ..Default::default()
        };
        for row in &rows {
            let image = known
                .get(&row.id)
                .filter(|(revision, _)| *revision == row.revision)
                .and_then(|(_, pixels)| pixels.clone());
            let state = if row.state == State::Ready && row.bytes.is_some() {
                State::Loading
            } else if row.state == State::Ready && image.is_none() {
                State::Invalid
            } else {
                row.state
            };
            result.rows.insert(row.id, Photo { state, image });
        }
        if let Some((id, _, Some(pixels))) = &viewer {
            result.viewer = Some((*id, pixels.clone()));
        }
        // Each publication checks for mutations and filters against the latest
        // demand. Scrolling must not discard a completed image still in view.
        let publish = |result: &Snapshot| {
            if archives.has_changed().unwrap_or(false)
                || changes
                    .as_ref()
                    .is_some_and(|c| c.has_changed().unwrap_or(true))
            {
                return false;
            }
            publish_current(&updates, result, &demand.borrow());
            true
        };
        if publish(&result) {
            for row in rows {
                if !demand.borrow().ids().contains(&row.id) {
                    continue;
                }
                let image = if let Some(bytes) = row.bytes {
                    decode(bytes, INLINE_EDGE).await.map(Arc::new)
                } else {
                    known.get(&row.id).and_then(|v| v.1.clone())
                };
                let state = if row.state == State::Ready && image.is_none() {
                    State::Invalid
                } else {
                    row.state
                };
                known.insert(row.id, (row.revision, image.clone()));
                result.rows.insert(row.id, Photo { state, image });
                if !publish(&result) {
                    break;
                }
                // Inline pixels are already usable while the larger viewer
                // decode is pending. Never decode an obsolete viewer request.
                if demand.borrow().viewer == Some(row.id) {
                    if let Some(original) = row.original {
                        let pixels = decode(original, VIEWER_EDGE).await.map(Arc::new);
                        viewer = Some((row.id, row.revision, pixels));
                    }
                    if let Some((id, _, Some(pixels))) = &viewer {
                        result.viewer = Some((*id, pixels.clone()));
                    }
                    if viewer.as_ref().is_some_and(|v| v.2.is_none())
                        && let Some(photo) = result.rows.get_mut(&row.id)
                    {
                        photo.state = State::Invalid;
                    }
                    if !publish(&result) {
                        break;
                    }
                }
            }
        }
        // The selected image has priority. Filmstrip work is bounded to seven
        // 96px cached previews and shares the same decoder/cache worker.
        if publish(&result) && demand.borrow().viewer == wanted.viewer {
            let keys = thumbs
                .iter()
                .map(|(id, (revision, _))| (*id, *revision))
                .collect();
            if let Ok(rows) = db.album_thumbnails(thumb_ids, keys).await {
                for (id, revision, bytes) in rows {
                    if demand.borrow().viewer != wanted.viewer {
                        break;
                    }
                    let pixels = decode(bytes, THUMB_EDGE).await.map(Thumbnail::new);
                    thumbs.insert(id, (revision, pixels.clone()));
                    if let Some(pixels) = pixels {
                        result.thumbnails.insert(id, pixels);
                    }
                    if !publish(&result) {
                        break;
                    }
                }
            }
        }
        // Requery after a mutation before publishing any more pixels. A demand
        // change needs only the new rows; completed overlapping rows are reused.
        first = !archives.has_changed().unwrap_or(false)
            && (demand.has_changed().unwrap_or(false)
                || changes
                    .as_ref()
                    .is_some_and(|c| c.has_changed().unwrap_or(false)));
    }
}

fn publish_current(updates: &watch::Sender<Arc<Snapshot>>, result: &Snapshot, wanted: &Demand) {
    let ids = wanted.ids();
    let result = Snapshot {
        rows: result
            .rows
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .map(|(id, p)| (*id, p.clone()))
            .collect(),
        viewer: result
            .viewer
            .as_ref()
            .filter(|(id, _)| wanted.viewer == Some(*id))
            .cloned(),
        checked: result
            .checked
            .filter(|&(chat, id)| wanted.chat == chat && wanted.viewer == Some(id)),
        gallery: result
            .gallery
            .as_ref()
            .filter(|g| wanted.viewer == Some(g.id) && result.checked == Some((wanted.chat, g.id)))
            .cloned(),
        thumbnails: if result.checked == wanted.viewer.map(|id| (wanted.chat, id)) {
            result.thumbnails.clone()
        } else {
            Default::default()
        },
    };
    updates.send_if_modified(|old| {
        let same_pixels = |a: &Option<Arc<Pixels>>, b: &Option<Arc<Pixels>>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        let same_viewer = match (&old.viewer, &result.viewer) {
            (None, None) => true,
            (Some((a, ap)), Some((b, bp))) => a == b && Arc::ptr_eq(ap, bp),
            _ => false,
        };
        if old.rows.len() == result.rows.len()
            && old.checked == result.checked
            && old.gallery == result.gallery
            && old.thumbnails.len() == result.thumbnails.len()
            && result
                .thumbnails
                .iter()
                .all(|(id, p)| old.thumbnails.get(id).is_some_and(|old| old.key == p.key))
            && same_viewer
            && result.rows.iter().all(|(id, p)| {
                old.rows
                    .get(id)
                    .is_some_and(|v| v.state == p.state && same_pixels(&v.image, &p.image))
            })
        {
            return false;
        }
        *old = Arc::new(result);
        true
    });
}
