//! One header update, independent of the chat/message models.
use crate::{AppWindow, presence::Snapshot};
use slint::ComponentHandle;
use tokio::sync::watch;

pub fn clear(ui: &AppWindow) {
    ui.set_chat_presence("".into());
    ui.set_chat_online(false);
}

pub fn bind(ui: &AppWindow, snapshots: watch::Receiver<Snapshot>) -> watch::Receiver<i32> {
    let (selected, receiver) = watch::channel(0);
    let weak = ui.as_weak();
    ui.on_request_presence(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let next = ui.get_presence_chat();
        selected.send_if_modified(|old| {
            if *old == next {
                false
            } else {
                *old = next;
                true
            }
        });
        // Reapply the latest slot when hide/show or a same-chat revision is
        // coalesced by the worker; never leave a cleared header stuck blank.
        publish(&ui, &snapshots.borrow());
    });
    ui.invoke_request_presence();
    receiver
}

pub fn publish(ui: &AppWindow, snapshot: &Snapshot) {
    if !ui.get_demo_mode()
        && ui.get_connection_state() == 5
        && snapshot.chat > 0
        && snapshot.chat == ui.get_selected_id()
        && snapshot.chat == ui.get_presence_chat()
    {
        ui.set_chat_presence(snapshot.label.as_str().into());
        ui.set_chat_online(snapshot.online);
    } else {
        clear(ui);
    }
}

pub fn conversation(ui: &AppWindow, chat: i32, group: bool) {
    if ui.get_demo_mode() {
        let label = match (group, chat) {
            (true, _) => "",
            (false, 1) => "online",
            _ => "last seen today at 10:25",
        };
        ui.set_chat_presence(label.into());
        ui.set_chat_online(label == "online");
    }
}

#[cfg(test)]
#[path = "presentation_presence_tests.rs"]
pub mod tests;
