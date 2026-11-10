//! Contact picking uses the existing bounded action channel and durable-save ACK.
#[cfg(test)]
#[path = "presentation_contact_sending_tests.rs"]
pub(crate) mod tests;
use crate::{
    AppWindow,
    contact_sending::Choice,
    message_actions::{Command, OperationReply},
    ui::SendContactRow,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;

fn selected(ui: &AppWindow) -> Vec<SendContactRow> {
    ui.get_send_contact_selected().iter().collect()
}
fn rows(ui: &AppWindow, contacts: &[Choice]) -> ModelRc<SendContactRow> {
    let selected = selected(ui);
    ModelRc::new(VecModel::from(
        contacts
            .iter()
            .map(|c| SendContactRow {
                name: c.name.as_str().into(),
                number: c.number.as_str().into(),
                initials: crate::domain::initials(&c.name).into(),
                checked: selected.iter().any(|s| s.number.as_str() == c.number),
            })
            .collect::<Vec<_>>(),
    ))
}
pub fn bind(ui: &AppWindow, submit: Rc<impl Fn(Command) -> bool + 'static>) {
    let weak = ui.as_weak();
    let load = submit.clone();
    ui.on_open_send_contacts(move || {
        let Some(ui) = weak.upgrade() else { return };
        if !ui.invoke_can_import()
            || ui.get_action_pending()
            || (!ui.get_demo_mode()
                && (ui.get_connection_state() != 5
                    || ui.get_live_revision() != ui.get_live_shown_revision()))
        {
            ui.invoke_show_error("Finish the current action before sharing contacts.".into());
            return;
        }
        crate::presentation_actions::request(&ui, &load, |request| Command::Contacts {
            query: String::new(),
            request,
        });
        if !ui.get_action_pending() {
            ui.invoke_show_error(ui.get_action_error());
            return;
        }
        ui.set_send_contact_chat(ui.get_selected_id());
        ui.set_send_contact_reply(ui.get_reply_id());
        ui.set_send_contact_query("".into());
        ui.set_send_contact_requested("".into());
        ui.set_send_contact_rows(Default::default());
        ui.set_send_contact_selected(Default::default());
        ui.set_send_contact_more(false);
        ui.set_send_contact_phase(1);
    });
    let weak = ui.as_weak();
    let load = submit.clone();
    ui.on_search_send_contacts(move |query| {
        let Some(ui) = weak.upgrade() else { return };
        ui.set_send_contact_query(query.clone());
        if ui.get_send_contact_phase() != 1 || ui.get_action_pending() {
            return;
        }
        crate::presentation_actions::request(&ui, &load, |request| Command::Contacts {
            query: query.into(),
            request,
        });
        if ui.get_action_pending() {
            ui.set_send_contact_requested(ui.get_send_contact_query());
        }
    });
    let weak = ui.as_weak();
    ui.on_toggle_send_contact(move |number| {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_send_contact_phase() == 0
            || (ui.get_send_contact_phase() == 2 && ui.get_action_pending())
        {
            return;
        }
        let mut choices = selected(&ui);
        if let Some(index) = choices.iter().position(|c| c.number == number) {
            choices.remove(index);
        } else if let Some(mut choice) = ui
            .get_send_contact_rows()
            .iter()
            .find(|r| r.number == number)
        {
            if choices.len() == crate::contact_sending::MAX_SELECTED {
                ui.set_action_error(
                    format!(
                        "Select up to {} contacts.",
                        crate::contact_sending::MAX_SELECTED
                    )
                    .into(),
                );
                return;
            }
            choice.checked = true;
            choices.push(choice);
        }
        ui.set_send_contact_selected(ModelRc::new(VecModel::from(choices)));
        let items = ui
            .get_send_contact_rows()
            .iter()
            .map(|r| Choice {
                name: r.name.to_string(),
                number: r.number.to_string(),
            })
            .collect::<Vec<_>>();
        ui.set_send_contact_rows(rows(&ui, &items));
        ui.invoke_request_avatars();
    });
    let weak = ui.as_weak();
    ui.on_confirm_send_contacts(move || {
        let Some(ui) = weak.upgrade() else { return };
        let contacts = selected(&ui)
            .into_iter()
            .map(|c| Choice {
                name: c.name.into(),
                number: c.number.into(),
            })
            .collect::<Vec<_>>();
        if contacts.is_empty() || ui.get_action_pending() {
            return;
        }
        if ui.get_send_contact_phase() == 1 {
            ui.set_send_contact_phase(2);
            ui.invoke_request_avatars();
            return;
        }
        if ui.get_send_contact_phase() != 2 {
            return;
        }
        if ui.get_send_contact_chat() != ui.get_selected_id() {
            ui.set_send_contact_phase(0);
            return;
        }
        if !ui.get_demo_mode() && ui.get_connection_state() != 5 {
            ui.set_action_error("Connect to WhatsApp to send these contacts.".into());
            return;
        }
        crate::presentation_actions::request(&ui, &submit, |request| Command::SendContacts {
            chat: ui.get_send_contact_chat(),
            contacts,
            reply: ui.get_send_contact_reply(),
            request,
        });
    });
}

pub fn publish(ui: &AppWindow, result: &OperationReply) -> bool {
    if !matches!(result.kind, 10 | 11) {
        return false;
    }
    if result.request != ui.get_action_request() || !ui.get_action_pending() {
        return true;
    }
    ui.set_action_pending(false);
    if ui.get_send_contact_phase() == 0 || ui.get_send_contact_chat() != ui.get_selected_id() {
        ui.set_send_contact_phase(0);
        return true;
    }
    if result.kind == 10 && ui.get_send_contact_query() != ui.get_send_contact_requested() {
        ui.invoke_search_send_contacts(ui.get_send_contact_query());
        return true;
    }
    if !result.saved {
        ui.set_action_error(result.error.as_str().into());
        return true;
    }
    if let Some(page) = result.contacts.as_ref().filter(|_| result.kind == 10) {
        ui.set_send_contact_rows(rows(ui, &page.choices));
        ui.set_send_contact_more(page.more);
        ui.invoke_request_avatars();
    } else if result.kind == 11 {
        ui.set_send_contact_phase(0);
        ui.set_send_contact_selected(Default::default());
        ui.set_send_contact_rows(Default::default());
        ui.invoke_request_avatars();
        ui.invoke_focus_composer();
    }
    true
}
