//! Retain immutable application rows; materialize Slint strings only on demand.
use super::chat_row;
use crate::{ChatRow, domain::ChatSummary};
use slint::{Model, ModelNotify, ModelTracker};
use std::{cell::RefCell, collections::BTreeMap, sync::Arc};

struct Rows {
    source: Arc<[ChatSummary]>,
    cache: Vec<Option<ChatRow>>,
    photos: BTreeMap<i32, (Arc<crate::avatars::Thumbnail>, slint::Image)>,
}

pub(crate) struct ChatListModel {
    rows: RefCell<Rows>,
    notify: ModelNotify,
}

impl ChatListModel {
    pub fn new(source: Arc<[ChatSummary]>) -> Self {
        Self {
            rows: RefCell::new(Rows {
                cache: vec![None; source.len()],
                source,
                photos: BTreeMap::new(),
            }),
            notify: ModelNotify::default(),
        }
    }

    pub fn update(&self, source: Arc<[ChatSummary]>) {
        let mut rows = self.rows.borrow_mut();
        if Arc::ptr_eq(&rows.source, &source) {
            return;
        }
        if rows.source.len() != source.len() {
            rows.cache = vec![None; source.len()];
            rows.source = source;
            drop(rows);
            self.notify.reset();
            return;
        }
        let changed: Vec<_> = rows
            .source
            .iter()
            .zip(source.iter())
            .enumerate()
            .filter_map(|(index, (old, new))| (old != new).then_some(index))
            .collect();
        for &index in &changed {
            rows.cache[index] = None;
        }
        rows.source = source;
        // Listeners can synchronously read the new data when notified.
        drop(rows);
        for index in changed {
            self.notify.row_changed(index);
        }
    }

    pub fn avatar_demand(&self, first: i32, count: i32, selected: i32) -> Vec<i32> {
        let rows = self.rows.borrow();
        let mut ids = vec![0];
        if selected > 0 {
            ids.push(selected);
        }
        for row in rows
            .source
            .iter()
            .skip(first.max(0) as usize)
            .take(count.clamp(0, 64) as usize)
        {
            if !ids.contains(&row.id) {
                ids.push(row.id);
            }
        }
        ids
    }

    pub fn avatar(&self, id: i32) -> slint::Image {
        self.rows
            .borrow()
            .photos
            .get(&id)
            .map(|(_, image)| image.clone())
            .unwrap_or_default()
    }

    pub fn update_avatars(&self, snapshot: &crate::avatars::Snapshot) {
        let mut rows = self.rows.borrow_mut();
        let mut photos = BTreeMap::new();
        for (id, pixels) in snapshot.iter() {
            let image = rows
                .photos
                .get(id)
                .filter(|(old, _)| Arc::ptr_eq(old, pixels))
                .map(|(_, image)| image.clone())
                .unwrap_or_else(|| {
                    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                        &pixels.pixels,
                        crate::avatars::EDGE,
                        crate::avatars::EDGE,
                    );
                    slint::Image::from_rgba8(buffer)
                });
            photos.insert(*id, (pixels.clone(), image));
        }
        let changed: Vec<_> = rows
            .source
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let same = match (rows.photos.get(&row.id), photos.get(&row.id)) {
                    (None, None) => true,
                    (Some((old, _)), Some((new, _))) => Arc::ptr_eq(old, new),
                    _ => false,
                };
                (!same).then_some(index)
            })
            .collect();
        for &index in &changed {
            rows.cache[index] = None;
        }
        rows.photos = photos;
        drop(rows);
        for index in changed {
            self.notify.row_changed(index);
        }
    }
}

impl Model for ChatListModel {
    type Data = ChatRow;

    fn row_count(&self) -> usize {
        self.rows.borrow().source.len()
    }

    fn row_data(&self, index: usize) -> Option<ChatRow> {
        let mut rows = self.rows.borrow_mut();
        let Rows {
            source,
            cache,
            photos,
        } = &mut *rows;
        let row = source.get(index)?;
        Some(
            cache[index]
                .get_or_insert_with(|| {
                    let mut item = chat_row(row);
                    if let Some((_, image)) = photos.get(&row.id) {
                        item.avatar = image.clone();
                    }
                    item
                })
                .clone(),
        )
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same_row(actual: ChatRow, expected: &ChatRow) {
        // Slint Image::None intentionally does not compare equal to itself.
        assert_eq!(
            (
                actual.id,
                actual.title,
                actual.initials,
                actual.tint,
                actual.preview,
                actual.time,
                actual.unread,
                actual.marked_unread
            ),
            (
                expected.id,
                expected.title.clone(),
                expected.initials.clone(),
                expected.tint,
                expected.preview.clone(),
                expected.time.clone(),
                expected.unread,
                expected.marked_unread
            )
        );
        assert_eq!(actual.avatar.size(), expected.avatar.size());
    }

    #[test]
    fn sidebar_cache_tracks_edits_reordering_filtering_and_empty_lists() {
        let original: Arc<[_]> = crate::fixtures::demo().sidebar().into();
        let model = ChatListModel::new(original.clone());
        let first = model.row_data(0).unwrap();
        let second = model.row_data(1).unwrap();
        assert!(model.row_data(original.len()).is_none());
        let mut next = original.to_vec();
        next[0].preview = "Updated\npreview".into();
        model.update(next.clone().into());
        assert_eq!(model.row_data(0).unwrap().preview, "Updated preview");
        same_row(model.row_data(1).unwrap(), &second);
        next.swap(0, 1);
        model.update(next.into());
        same_row(model.row_data(0).unwrap(), &second);
        assert_eq!(model.row_data(1).unwrap().id, first.id);
        model.update(vec![original[0].clone()].into());
        assert_eq!(model.row_count(), 1);
        same_row(model.row_data(0).unwrap(), &first);
        assert!(model.row_data(1).is_none());
        model.update(Arc::from([]));
        assert_eq!(model.row_count(), 0);
        assert!(model.row_data(0).is_none());
        model.update(original);
        same_row(model.row_data(0).unwrap(), &first);
    }
}
