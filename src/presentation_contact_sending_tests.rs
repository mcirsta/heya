use crate::{
    AppWindow, Theme, contact_sending, fixtures,
    message_actions::{Command, OperationReply},
    presentation, presentation_actions,
};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, rc::Rc, sync::Arc};

fn finish(ui: &AppWindow, kind: u8, saved: bool, page: Option<contact_sending::Page>) {
    presentation_actions::publish(
        ui,
        Some(&OperationReply {
            request: ui.get_action_request(),
            kind,
            saved,
            contacts: page,
            error: if saved {
                String::new()
            } else {
                "Synthetic save failed; selection retained.".into()
            },
            ..Default::default()
        }),
        &Arc::from([]),
    );
}
fn contains(parent: &ElementHandle, child: &ElementHandle) {
    let a = parent.absolute_position();
    let b = child.absolute_position();
    let p = parent.size();
    let c = child.size();
    assert!(
        b.x >= a.x - 0.5
            && b.y >= a.y - 0.5
            && b.x + c.width <= a.x + p.width + 0.5
            && b.y + c.height <= a.y + p.height + 0.5
    );
}
pub(crate) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let mut demo = fixtures::demo();
    demo.select(9);
    presentation::conversation(&ui, &demo.conversation(), true);
    ui.set_draft("Keep my composer text 👩🏽‍💻".into());
    let commands = Rc::new(RefCell::new(Vec::new()));
    presentation_actions::bind(&ui, {
        let commands = commands.clone();
        move |c| {
            commands.borrow_mut().push(c);
            true
        }
    });
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    ui.set_attachment_menu(true);
    ui.set_attachment_x(340.);
    ui.set_attachment_y(800.);
    ElementHandle::find_by_accessible_label(&ui, "Contact")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(ui.get_send_contact_phase(), 1);
    assert!(ui.get_action_pending());
    assert_eq!(commands.borrow().len(), 1);
    assert!(matches!(commands.borrow()[0], Command::Contacts { .. }));
    ui.invoke_search_send_contacts("Mî".into());
    ui.invoke_search_send_contacts("MÎRA".into());
    assert_eq!(
        commands.borrow().len(),
        1,
        "Typing during a query retains only the newest query"
    );
    finish(&ui, 10, true, Some(contact_sending::demo("")));
    assert_eq!(commands.borrow().len(), 2);
    assert!(ui.get_action_pending());
    finish(&ui, 10, true, Some(contact_sending::demo("MÎRA")));
    assert_eq!(ui.get_send_contact_rows().row_count(), 1);
    assert!(!ui.get_action_pending());
    ui.invoke_toggle_send_contact("15550000002".into());
    ui.invoke_search_send_contacts("Alex".into());
    finish(&ui, 10, true, Some(contact_sending::demo("Alex")));
    assert_eq!(
        ui.get_send_contact_selected().row_count(),
        1,
        "Search must preserve selection"
    );
    ui.invoke_toggle_send_contact("15550000001".into());
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for (w, h) in [(1200., 900.), (520., 480.)] {
            ui.window().set_size(slint::LogicalSize::new(w, h));
            let dialog = ElementHandle::find_by_element_type_name(&ui, "SendContactsDialog")
                .next()
                .unwrap();
            let panel = dialog
                .query_descendants()
                .match_id("SendContactsDialog::panel")
                .find_first()
                .unwrap();
            contains(&dialog, &panel);
            for id in [
                "SendContactsDialog::header",
                "SendContactsDialog::body",
                "SendContactsDialog::footer",
            ] {
                contains(
                    &panel,
                    &panel.query_descendants().match_id(id).find_first().unwrap(),
                );
            }
        }
    }
    ui.invoke_confirm_send_contacts();
    assert_eq!(ui.get_send_contact_phase(), 2);
    ui.invoke_confirm_send_contacts();
    let sent = commands.borrow().len();
    ui.invoke_confirm_send_contacts();
    assert_eq!(
        commands.borrow().len(),
        sent,
        "Double click must submit once"
    );
    assert!(
        matches!(commands.borrow().last(),Some(Command::SendContacts{chat:9,contacts,..}) if contacts.len()==2)
    );
    assert_eq!(ui.get_draft(), "Keep my composer text 👩🏽‍💻");
    finish(&ui, 11, false, None);
    assert_eq!(ui.get_send_contact_phase(), 2);
    assert_eq!(ui.get_send_contact_selected().row_count(), 2);
    assert!(!ui.get_action_error().is_empty());
    ui.invoke_confirm_send_contacts();
    assert!(ui.get_action_pending());
    ui.set_selected_id(8);
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_send_contact_phase(), 0);
    finish(&ui, 11, true, None);
    assert_eq!(ui.get_send_contact_phase(), 0);
    assert_eq!(ui.get_selected_id(), 8);
    presentation_actions::bind(&ui, |_| false);
    ui.invoke_open_send_contacts();
    assert_eq!(ui.get_send_contact_phase(), 0);
    assert!(!ui.get_action_pending());
    assert!(!ui.get_error_message().is_empty());
    ui.hide().unwrap();
}
