//! Keep album identities stable; only the bounded preview cache owns textures.
use crate::{photos, ui::PhotoThumbnail};
use slint::{Model, ModelNotify, ModelTracker};
use std::{cell::RefCell, collections::BTreeMap};

pub(super) struct StripModel {
    ids: Vec<i32>,
    positions: BTreeMap<i32, usize>,
    images: RefCell<BTreeMap<i32, PhotoThumbnail>>,
    notify: ModelNotify,
}

impl StripModel {
    pub fn new(ids: &[i32]) -> Self {
        Self {
            ids: ids.to_vec(),
            positions: ids.iter().enumerate().map(|(i, &id)| (id, i)).collect(),
            images: Default::default(),
            notify: Default::default(),
        }
    }

    pub fn update(&self, thumbnails: &BTreeMap<i32, photos::Thumbnail>) {
        let mut images = self.images.borrow_mut();
        let mut changed = Vec::new();
        images.retain(|id, _| {
            if thumbnails.contains_key(id) {
                true
            } else {
                changed.push(self.positions[id]);
                false
            }
        });
        for (&id, thumbnail) in thumbnails {
            let Some(&index) = self.positions.get(&id) else {
                continue;
            };
            let key = thumbnail.key.to_string();
            if images.get(&id).is_some_and(|old| old.key == key) {
                continue;
            }
            let pixels = &thumbnail.pixels;
            images.insert(
                id,
                PhotoThumbnail {
                    id,
                    key: key.into(),
                    picture: slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                        &pixels.rgba,
                        pixels.width,
                        pixels.height,
                    )),
                },
            );
            changed.push(index);
        }
        // Model notifications may synchronously read the updated row.
        drop(images);
        for index in changed {
            self.notify.row_changed(index);
        }
    }
}

impl Model for StripModel {
    type Data = PhotoThumbnail;
    fn row_count(&self) -> usize {
        self.ids.len()
    }
    fn row_data(&self, index: usize) -> Option<PhotoThumbnail> {
        let &id = self.ids.get(index)?;
        Some(
            self.images
                .borrow()
                .get(&id)
                .cloned()
                .unwrap_or(PhotoThumbnail {
                    id,
                    ..Default::default()
                }),
        )
    }
    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
