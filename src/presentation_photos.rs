//! Native photo controls publish bounded demand; no decoding on the event loop.
use crate::{AppWindow, photos, presentation::timeline};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::Cell, rc::Rc, sync::Arc};
use tokio::sync::watch;

#[cfg(test)]
#[path = "presentation_photo_tests.rs"]
mod tests;
#[cfg(test)]
pub(crate) use tests::check_ui;

pub fn bind(ui: &AppWindow) -> watch::Receiver<photos::Demand> {
    let (demand, receiver) = watch::channel(photos::Demand::default());
    let serial = Rc::new(Cell::new(0_u64));
    let weak = ui.as_weak();
    let tx = demand.clone();
    ui.on_request_photos(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let viewer = (ui.get_photo_viewer_id() > 0).then_some(ui.get_photo_viewer_id());
        timeline::refresh_photo_viewer(&ui);
        let visible = ui
            .get_timeline_rows()
            .iter()
            .filter(|r| r.message.has_photo)
            .map(|r| r.message.id)
            .take(photos::MAX_DEMAND)
            .collect();
        tx.send_if_modified(|old| {
            let next = photos::Demand {
                chat: ui.get_selected_id(),
                visible,
                viewer,
                automatic: ui.get_auto_download_photos(),
                retry: old.retry,
            };
            if *old == next {
                return false;
            }
            *old = next;
            true
        });
    });
    let weak = ui.as_weak();
    let tx = demand.clone();
    let retry_serial = serial.clone();
    ui.on_activate_photo(move |id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if timeline::has_photo(&ui, id) || (ui.get_photo_viewer_id() == id && id > 0) {
            let cached_or_busy = ui
                .get_timeline_rows()
                .iter()
                .find(|r| r.message.id == id)
                .is_some_and(|r| matches!(r.message.photo_state, 1 | 2))
                || (ui.get_photo_viewer_id() == id && ui.get_photo_viewer_ready());
            open(&ui, id, &tx, &retry_serial, !cached_or_busy);
        }
    });
    let weak = ui.as_weak();
    ui.on_navigate_photo(move |id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_photo_viewer_id() > 0
            && ui.get_photo_gallery_ids().iter().any(|member| member == id)
        {
            open(&ui, id, &demand, &serial, false);
        }
    });
    ui.invoke_request_photos();
    receiver
}

fn open(
    ui: &AppWindow,
    id: i32,
    tx: &watch::Sender<photos::Demand>,
    serial: &Cell<u64>,
    retry: bool,
) {
    if ui.get_photo_viewer_id() != id {
        ui.set_photo_viewer_caption(Default::default());
        ui.set_photo_viewer_date(Default::default());
    }
    ui.set_photo_viewer_id(id);
    ui.invoke_request_photos();
    if retry {
        serial.set(serial.get().wrapping_add(1));
        tx.send_modify(|v| v.retry = Some((id, serial.get())));
    }
    // Navigation remains responsive while decoding: the complete ID list is
    // small metadata, while only seven thumbnails become UI elements.
    let ids = ui.get_photo_gallery_ids();
    if let Some(position) = ids.iter().position(|member| member == id) {
        ui.set_photo_viewer_previous(
            position
                .checked_sub(1)
                .and_then(|p| ids.row_data(p))
                .unwrap_or(0),
        );
        ui.set_photo_viewer_next(ids.row_data(position + 1).unwrap_or(0));
        ui.set_photo_viewer_counter(
            format!("Photo {} of {}", position + 1, ids.row_count()).into(),
        );
    }
}

pub fn publish(ui: &AppWindow, snapshot: &Arc<photos::Snapshot>) {
    timeline::photos(ui, snapshot);
    let id = ui.get_photo_viewer_id();
    if snapshot.checked != Some((ui.get_selected_id(), id)) {
        return;
    }
    let Some(gallery) = &snapshot.gallery else {
        ui.invoke_close_photo();
        return;
    };
    ui.set_photo_viewer_sender(gallery.sender.clone().into());
    ui.set_photo_viewer_date(gallery.date.clone().into());
    ui.set_photo_viewer_caption(gallery.caption.clone().into());
    ui.set_photo_viewer_counter(if gallery.members.len() > 1 {
        format!(
            "Photo {} of {}",
            gallery.position + 1,
            gallery.members.len()
        )
        .into()
    } else {
        "Photo".into()
    });
    let ids = ui.get_photo_gallery_ids();
    if ids.row_count() != gallery.members.len() || ids.iter().ne(gallery.members.iter().copied()) {
        ui.set_photo_gallery_ids(ModelRc::new(VecModel::from(gallery.members.clone())));
    }
    ui.set_photo_viewer_previous(
        gallery
            .position
            .checked_sub(1)
            .map(|i| gallery.members[i])
            .unwrap_or(0),
    );
    ui.set_photo_viewer_next(
        gallery
            .members
            .get(gallery.position + 1)
            .copied()
            .unwrap_or(0),
    );
    let old = ui.get_photo_thumbnails();
    let thumbnails = if gallery.members.len() > 1 {
        gallery.thumbnails()
    } else {
        &[]
    };
    let rows = thumbnails
        .iter()
        .map(|&id| {
            let pixels = snapshot.thumbnails.get(&id);
            // Pixel identity changes only on a new decode. Retain the Slint image
            // so publications for loading/status don't upload its texture again.
            let key = pixels.map(|p| p.key.to_string()).unwrap_or_default();
            let picture = old
                .iter()
                .find(|row| row.id == id && row.key == key)
                .map(|row| row.picture)
                .unwrap_or_else(|| {
                    pixels
                        .map(|p| {
                            slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                                &p.pixels.rgba,
                                p.pixels.width,
                                p.pixels.height,
                            ))
                        })
                        .unwrap_or_default()
                });
            crate::ui::PhotoThumbnail {
                id,
                picture,
                key: key.into(),
            }
        })
        .collect::<Vec<_>>();
    if rows.len() != old.row_count()
        || rows
            .iter()
            .zip(old.iter())
            .any(|(a, b)| a.id != b.id || a.key != b.key)
    {
        ui.set_photo_thumbnails(ModelRc::new(VecModel::from(rows)));
    }
}
