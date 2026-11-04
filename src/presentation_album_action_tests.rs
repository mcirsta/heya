//! Album card actions through the real handlers: explicit albums react once to
//! the album message, inferred albums react to the hovered photo.
use crate::{AppWindow, fixtures, message_actions::Command, presentation};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, rc::Rc};

fn child(parent: &ElementHandle, id: &str) -> ElementHandle {
    parent
        .query_descendants()
        .match_id(id)
        .find_first()
        .unwrap_or_else(|| panic!("missing {id}"))
}

fn card(ui: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_type_name(ui, "TimelineItem")
        .find(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
        .unwrap()
}

/// Hover the fourth tile, which is a hidden member of the card.
fn hover_last_tile(ui: &AppWindow) -> ElementHandle {
    let item = card(ui);
    let tiles = child(&item, "MessageBubble::bubble")
        .query_descendants()
        .match_type_name("PhotoTile")
        .find_all();
    let p = tiles[3].absolute_position();
    let s = tiles[3].size();
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
        });
    item
}

fn react(ui: &AppWindow, pending: &RefCell<Vec<Command>>, label: &str) -> Command {
    let item = hover_last_tile(ui);
    child(&item, "MessageHoverControls::react").invoke_accessible_default_action();
    assert_eq!(
        ui.get_message_dialog(),
        1,
        "React on a later tile opens the picker"
    );
    assert!(pending.borrow().is_empty());
    ui.invoke_search_message_actions("thumbs up".into());
    ElementHandle::find_by_accessible_label(ui, label)
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    pending.borrow_mut().pop().expect("one reaction command")
}

pub(crate) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    let pending = Rc::new(RefCell::new(Vec::new()));
    crate::presentation_actions::bind(&ui, {
        let pending = pending.clone();
        move |command| {
            pending.borrow_mut().push(command);
            true
        }
    });
    let mut demo = fixtures::demo();
    demo.select(9);
    let seed = demo.conversation().messages[0].clone();
    for unlinked in [false, true] {
        let chat = demo.chats.iter_mut().find(|c| c.id == 9).unwrap();
        chat.messages = if unlinked {
            fixtures::unlinked_album_messages(&seed, 38)
        } else {
            fixtures::album_messages(&seed, 38)
        };
        presentation::conversation(&ui, &demo.conversation(), true);
        ui.show().unwrap();
        ui.invoke_layout_timeline();
        let hovered = 44003;

        // Reply from a later tile quotes that photo in both album kinds.
        let item = hover_last_tile(&ui);
        child(&item, "MessageHoverControls::reply").invoke_accessible_default_action();
        let command = pending.borrow_mut().pop();
        assert!(matches!(command, Some(Command::Reply {message,..}) if message==hovered));
        let (result, targets) = demo.message_action(command.unwrap());
        presentation::conversation(&ui, &demo.conversation(), false);
        crate::presentation_actions::publish(&ui, Some(&result), &targets);
        ui.invoke_cancel_reply();
        let (result, targets) = demo.message_action(pending.borrow_mut().pop().unwrap());
        presentation::conversation(&ui, &demo.conversation(), false);
        crate::presentation_actions::publish(&ui, Some(&result), &targets);
        ui.invoke_layout_timeline();

        let command = react(&ui, &pending, "React with thumbs up");
        assert!(
            matches!(&command, Command::React {message,emoji,album,..}
                if *message==hovered && emoji=="👍" && *album==!unlinked),
            "explicit albums react to the album, inferred ones to the photo"
        );
        let (result, targets) = demo.message_action(command);
        presentation::conversation(&ui, &demo.conversation(), false);
        crate::presentation_actions::publish(&ui, Some(&result), &targets);
        assert_eq!(ui.get_message_dialog(), 0);
        ui.invoke_layout_timeline();
        let rows: Vec<_> = ui.get_timeline_rows().iter().collect();
        if unlinked {
            // A reacted photo leaves an inferred card, as before.
            assert!(
                rows.iter()
                    .any(|r| r.message.id == hovered && r.message.reactions == "👍")
            );
            continue;
        }
        assert_eq!(rows.len(), 1, "the explicit album stays one card");
        assert!(rows[0].message.album_target);
        assert_eq!(rows[0].message.reactions, "👍");
        // Choosing the same emoji again removes the album reaction.
        let command = react(&ui, &pending, "Remove your reaction");
        assert!(matches!(&command, Command::React {emoji,album,..} if emoji.is_empty() && *album));
        let (result, targets) = demo.message_action(command);
        presentation::conversation(&ui, &demo.conversation(), false);
        crate::presentation_actions::publish(&ui, Some(&result), &targets);
        ui.invoke_layout_timeline();
        assert_eq!(
            ui.get_timeline_rows()
                .row_data(0)
                .unwrap()
                .message
                .reactions,
            ""
        );
        // The right-click menu's reaction strip reacts to the album too.
        ui.invoke_message_action(hovered, 17, 600., 400.);
        assert_eq!(ui.get_message_dialog(), 3);
        ElementHandle::find_by_accessible_label(&ui, "React with 👍")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        let command = pending.borrow_mut().pop();
        assert!(
            matches!(&command, Some(Command::React {message,emoji,album,..}) if *message==hovered && emoji=="👍" && *album)
        );
        let (result, targets) = demo.message_action(command.unwrap());
        presentation::conversation(&ui, &demo.conversation(), false);
        crate::presentation_actions::publish(&ui, Some(&result), &targets);
    }
    ui.hide().unwrap();
}
