//! Small UI handoff: receives decoded pixels; performs no disk/network/decoding.
use crate::{AppWindow, avatars, presentation::chat_list::ChatListModel};
use slint::{ComponentHandle, Model};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::watch;

/// Both lists retain texture identity; only newly decoded thumbnails are copied.
#[derive(Default)]
pub(crate) struct Images(BTreeMap<avatars::Target, (Arc<avatars::Thumbnail>, slint::Image)>);

impl Images {
    pub fn get(&self, target: &avatars::Target) -> slint::Image {
        self.0
            .get(target)
            .map(|(_, image)| image.clone())
            .unwrap_or_default()
    }

    pub fn update(
        &mut self,
        snapshot: &avatars::Snapshot,
        participants: bool,
    ) -> Vec<avatars::Target> {
        let mut next = BTreeMap::new();
        let mut changed = Vec::new();
        for (target, pixels) in snapshot.iter().take(avatars::MAX_DEMAND) {
            if matches!(target, avatars::Target::Participant(_)) != participants {
                continue;
            }
            let image = self
                .0
                .get(target)
                .filter(|(old, _)| Arc::ptr_eq(old, pixels))
                .map(|(_, image)| image.clone())
                .unwrap_or_else(|| {
                    changed.push(target.clone());
                    slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                        &pixels.pixels,
                        avatars::EDGE,
                        avatars::EDGE,
                    ))
                });
            next.insert(target.clone(), (pixels.clone(), image));
        }
        changed.extend(self.0.keys().filter(|id| !next.contains_key(*id)).cloned());
        self.0 = next;
        changed
    }
}

pub fn bind(ui: &AppWindow) -> watch::Receiver<Vec<avatars::Target>> {
    // Establish the model before either the archive or avatar snapshot arrives.
    crate::presentation::sidebar(ui, std::sync::Arc::from([]));
    let (demand, receiver) = watch::channel(vec![avatars::Target::Chat(0)]);
    let weak = ui.as_weak();
    ui.on_request_avatars(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let model = ui.get_chats();
        let Some(model) = model.as_any().downcast_ref::<ChatListModel>() else {
            return;
        };
        let chats = model.avatar_demand(
            ui.get_avatar_first(),
            ui.get_avatar_count(),
            ui.get_selected_id(),
        );
        // Prioritize the account/open chat, then the small materialized group
        // window. Never walk all loaded messages or the entire membership list.
        let mut ids = vec![avatars::Target::Chat(0)];
        if ui.get_selected_id() > 0 {
            ids.push(avatars::Target::Chat(ui.get_selected_id()));
        }
        if ui.get_chat_is_group() && ui.get_history_active() {
            for row in ui.get_timeline_rows().iter().take(avatars::MAX_DEMAND) {
                let m = row.message;
                if m.outgoing || !m.first_in_run || m.system_notice || m.sender_id.is_empty() {
                    continue;
                }
                let target = avatars::Target::Participant(m.sender_id.to_string());
                if !ids.contains(&target) && ids.len() < avatars::MAX_DEMAND {
                    ids.push(target);
                }
            }
        }
        for chat in chats {
            let target = avatars::Target::Chat(chat);
            if !ids.contains(&target) && ids.len() < avatars::MAX_DEMAND {
                ids.push(target);
            }
        }
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
    crate::presentation::timeline::avatars(ui, snapshot);
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
    assert_eq!(
        *demand.borrow(),
        vec![0.into(), rows[1].id.into(), rows[0].id.into()]
    );
    let photo = Arc::new(avatars::Thumbnail::from_pixels(vec![39; 65536]).unwrap());
    let snapshot = Arc::new(BTreeMap::from([
        (0.into(), photo.clone()),
        (rows[0].id.into(), photo),
    ]));
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
    check_group_ui();
}

#[cfg(test)]
fn check_group_ui() {
    use crate::{fixtures, presentation};
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let demand = bind(&ui);
    let mut demo = fixtures::demo();
    demo.select(2);
    let mut view = demo.conversation();
    let first = view.messages[0].clone();
    // Same display name, separate identities, repeated runs, outgoing and
    // system rows; a long history must demand only its materialized window.
    view.messages = (0..1000)
        .map(|i| {
            let mut m = first.clone();
            m.id = i + 1;
            m.sender = "~Alex".into();
            m.sender_id = format!("demo:participant-{}", i / 2);
            m.outgoing = i % 7 == 0;
            if i % 11 == 0 {
                Arc::make_mut(&mut m.content).notice =
                    Some(crate::message_content::Notice::MessagePinned);
            }
            m
        })
        .collect();
    for (width, height) in [(1200., 800.), (520., 700.)] {
        ui.window().set_size(slint::LogicalSize::new(width, height));
        presentation::conversation(&ui, &view, true);
        ui.invoke_layout_timeline();
        let visible: Vec<_> = ui.get_timeline_rows().iter().collect();
        assert!(visible.len() < 30);
        let participants: Vec<_> = demand
            .borrow()
            .iter()
            .filter_map(|target| match target {
                avatars::Target::Participant(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert!(!participants.is_empty());
        assert!(participants.len() < 20);
        assert!(
            participants
                .iter()
                .all(
                    |id| visible.iter().any(|r| r.message.sender_id == id.as_str()
                        && r.message.first_in_run
                        && !r.message.outgoing
                        && !r.message.system_notice)
                )
        );
        let target = avatars::Target::Participant(participants[0].clone());
        let snapshot = Arc::new(BTreeMap::from([(
            target.clone(),
            Arc::new(avatars::Thumbnail::from_pixels(vec![39; 65536]).unwrap()),
        )]));
        let geometry = (ui.get_timeline_height(), ui.get_timeline_y());
        presentation::timeline::take_samples();
        publish(&ui, &snapshot);
        ui.invoke_layout_timeline();
        assert_eq!(presentation::timeline::take_samples().measured_rows, 0);
        assert_eq!(geometry, (ui.get_timeline_height(), ui.get_timeline_y()));
        let mut photo = slint::Image::default();
        for row in ui.get_timeline_rows().iter() {
            let m = row.message;
            assert_eq!(m.sender_initials, "A");
            let expected = m.sender_id == participants[0].as_str()
                && m.first_in_run
                && !m.outgoing
                && !m.system_notice;
            assert_eq!(m.sender_avatar.size().width, if expected { 128 } else { 0 });
            if expected {
                photo = m.sender_avatar;
            }
        }
        assert_eq!(photo.size().width, 128);
        publish(&ui, &snapshot);
        let repeated = ui
            .get_timeline_rows()
            .iter()
            .find(|r| r.message.sender_avatar.size().width > 0)
            .unwrap();
        assert_eq!(
            photo, repeated.message.sender_avatar,
            "reuse the same texture"
        );
        publish(&ui, &avatars::Snapshot::default());
        assert!(
            ui.get_timeline_rows()
                .iter()
                .all(|r| r.message.sender_avatar.size().width == 0)
        );
        assert_eq!(geometry, (ui.get_timeline_height(), ui.get_timeline_y()));
    }
    demo.select(1);
    presentation::conversation(&ui, &demo.conversation(), true);
    ui.invoke_request_avatars();
    assert!(
        demand
            .borrow()
            .iter()
            .all(|t| matches!(t, avatars::Target::Chat(_)))
    );
}
