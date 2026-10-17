//! Small UI handoff: receives decoded pixels; performs no disk/network/decoding.
use crate::{AppWindow, avatars, presentation::chat_list::ChatListModel};
use slint::{ComponentHandle, Model};
use tokio::sync::watch;

pub fn bind(ui: &AppWindow) -> watch::Receiver<Vec<i32>> {
    // Establish the model before either the archive or avatar snapshot arrives.
    crate::presentation::sidebar(ui, std::sync::Arc::from([]));
    let (demand, receiver) = watch::channel(vec![0]);
    let weak = ui.as_weak();
    ui.on_request_avatars(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let model = ui.get_chats();
        let Some(model) = model.as_any().downcast_ref::<ChatListModel>() else {
            return;
        };
        let ids = model.avatar_demand(
            ui.get_avatar_first(),
            ui.get_avatar_count(),
            ui.get_selected_id(),
        );
        demand.send_if_modified(|old| {
            if *old == ids {
                false
            } else {
                *old = ids;
                true
            }
        });
        refresh(&ui);
    });
    receiver
}

pub fn publish(ui: &AppWindow, snapshot: &avatars::Snapshot) {
    let model = ui.get_chats();
    if let Some(model) = model.as_any().downcast_ref::<ChatListModel>() {
        model.update_avatars(snapshot);
        refresh(ui);
    }
}

fn same_image(a: &slint::Image, b: &slint::Image) -> bool {
    // Slint's empty Image is not equal to itself. Avoid repeatedly invalidating
    // the header when a chat has no photo and unrelated snapshots arrive.
    (a.size().width == 0 && b.size().width == 0) || a == b
}

pub(crate) fn refresh(ui: &AppWindow) {
    let rows = ui.get_chats();
    let Some(model) = rows.as_any().downcast_ref::<ChatListModel>() else {
        return;
    };
    let chat = model.avatar(ui.get_selected_id());
    if !same_image(&ui.get_chat_avatar(), &chat) {
        ui.set_chat_avatar(chat);
    }
    let account = model.avatar(0);
    if !same_image(&ui.get_account_avatar(), &account) {
        ui.set_account_avatar(account);
    }
}

#[cfg(test)]
pub(crate) fn check_ui() {
    use std::{collections::BTreeMap, sync::Arc};
    let ui = AppWindow::new().unwrap();
    assert!(same_image(
        &slint::Image::default(),
        &slint::Image::default()
    ));
    let demand = bind(&ui);
    let rows: Arc<[_]> = crate::fixtures::demo().sidebar().into();
    crate::presentation::sidebar(&ui, rows.clone());
    ui.set_selected_id(rows[1].id);
    ui.set_avatar_first(0);
    ui.set_avatar_count(2);
    ui.invoke_request_avatars();
    assert_eq!(*demand.borrow(), vec![0, rows[1].id, rows[0].id]);
    let photo = Arc::new(avatars::Thumbnail::from_pixels(vec![39; 65536]).unwrap());
    let snapshot = Arc::new(BTreeMap::from([(0, photo.clone()), (rows[0].id, photo)]));
    publish(&ui, &snapshot);
    assert_eq!(ui.get_account_avatar().size().width, 128);
    assert_eq!(ui.get_chat_avatar().size().width, 0);
    ui.set_selected_id(rows[0].id);
    ui.invoke_request_avatars();
    assert_eq!(ui.get_chat_avatar().to_rgba8().unwrap().as_bytes()[0], 39);
    let mut reordered = rows.to_vec();
    reordered.swap(0, 1);
    crate::presentation::sidebar(&ui, reordered);
    assert_eq!(ui.get_chats().row_data(0).unwrap().avatar.size().width, 0);
    assert_eq!(ui.get_chats().row_data(1).unwrap().avatar.size().width, 128);
    let model = ui.get_chats();
    let model = model.as_any().downcast_ref::<ChatListModel>().unwrap();
    model.update(
        (1..=500)
            .map(|id| crate::domain::ChatSummary {
                id,
                ..rows[0].clone()
            })
            .collect(),
    );
    let bounded = model.avatar_demand(400, i32::MAX, 1);
    assert_eq!(bounded.len(), avatars::MAX_DEMAND);
    assert_eq!(&bounded[..3], &[0, 1, 401]);
    assert_eq!(model.avatar_demand(9999, 20, 1), vec![0, 1]);
    publish(&ui, &avatars::Snapshot::default());
    assert_eq!(ui.get_chat_avatar().size().width, 0);
    assert_eq!(ui.get_account_avatar().size().width, 0);
    assert_eq!(ui.get_chats().row_data(1).unwrap().avatar.size().width, 0);
}
