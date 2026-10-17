//! Native photo controls publish bounded demand; no decoding on the event loop.
use crate::{AppWindow, photos, presentation::timeline};
use slint::{ComponentHandle, Model};
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
        let mut viewer = (ui.get_photo_viewer_id() > 0).then_some(ui.get_photo_viewer_id());
        if viewer.is_some_and(|id| !timeline::has_photo(&ui, id)) {
            ui.invoke_close_photo();
            // The callback is already running; finish publishing the cleared
            // demand here instead of relying on a recursive callback.
            viewer = None;
        }
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
    ui.on_activate_photo(move |id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !timeline::has_photo(&ui, id) {
            return;
        }
        ui.set_photo_viewer_id(id);
        ui.invoke_request_photos();
        let busy_or_ready = ui
            .get_timeline_rows()
            .iter()
            .find(|r| r.message.id == id)
            .is_some_and(|r| matches!(r.message.photo_state, 1 | 2));
        if !busy_or_ready {
            serial.set(serial.get().wrapping_add(1));
            demand.send_modify(|v| v.retry = Some((id, serial.get())));
        }
    });
    ui.invoke_request_photos();
    receiver
}

pub fn publish(ui: &AppWindow, snapshot: &Arc<photos::Snapshot>) {
    timeline::photos(ui, snapshot);
}
