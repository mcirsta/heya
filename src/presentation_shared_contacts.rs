//! User-clicked shared contacts reuse the normal new-chat and download handoffs.
use crate::{AppWindow, presentation::timeline};
use slint::{ComponentHandle, ModelRc, VecModel};

fn message(ui: &AppWindow, number: &str) {
    if ui.get_demo_mode() || ui.get_connection_state() != 5 {
        ui.invoke_show_error("Connect to WhatsApp to message this contact.".into());
        return;
    }
    if crate::domain::phone_number(number).is_none() {
        ui.invoke_show_error("This contact has no WhatsApp phone number.".into());
        return;
    }
    ui.set_shared_contacts_open(false);
    ui.set_new_chat_open(true);
    ui.invoke_new_chat(number.into());
}
pub fn open(ui: &AppWindow, id: i32, send: bool) {
    let Some(content) = timeline::shared_contacts(ui, id) else {
        return;
    };
    if send {
        message(ui, content.contacts[0].account());
        return;
    }
    let rows = content
        .contacts
        .iter()
        .map(|c| crate::ui::SharedContactRow {
            name: c.name.as_str().into(),
            initials: crate::domain::initials(&c.name).into(),
            account: c.account().into(),
            details: c.details().into(),
        })
        .collect::<Vec<_>>();
    ui.set_shared_contact_selected(if rows.len() == 1 { 0 } else { -1 });
    ui.set_shared_contact_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_shared_contacts_message(id);
    ui.set_shared_contacts_open(true);
    ui.invoke_video_control(0, 3, 0.);
}
pub fn refresh(ui: &AppWindow) {
    if ui.get_shared_contacts_open()
        && timeline::shared_contacts(ui, ui.get_shared_contacts_message()).is_none()
    {
        ui.set_shared_contacts_open(false);
        ui.set_shared_contact_rows(Default::default());
    }
}
pub fn bind(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_shared_contact_action(move |index, kind| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let id = ui.get_shared_contacts_message();
        let Some(content) = timeline::shared_contacts(&ui, id) else {
            refresh(&ui);
            return;
        };
        let Some(contact) = usize::try_from(index)
            .ok()
            .and_then(|i| content.contacts.get(i))
        else {
            return;
        };
        match kind {
            1 => message(&ui, contact.account()),
            2 => ui.invoke_copy_value(format!("{}\n{}", contact.name, contact.details()).into()),
            3 => ui.invoke_download_message(ui.get_selected_id(), id),
            _ => {}
        }
    });
}
