//! Synthetic menu input/layout checks; no account, network or desktop window.
use crate::{AppWindow, Theme, fixtures, presentation};
use i_slint_backend_testing::ElementHandle;
use slint::{
    ComponentHandle, Model,
    platform::{PointerEventButton, WindowEvent},
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

fn element(ui: &AppWindow, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, id)
        .next()
        .unwrap_or_else(|| panic!("missing {id}"))
}
fn center(e: &ElementHandle) -> slint::LogicalPosition {
    let p = e.absolute_position();
    let s = e.size();
    slint::LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.)
}
fn click(ui: &AppWindow, p: slint::LogicalPosition) {
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: p });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: p,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: p,
        button: PointerEventButton::Left,
    });
    slint::platform::update_timers_and_animations();
}
fn key(ui: &AppWindow, key: slint::SharedString) {
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key });
    slint::platform::update_timers_and_animations();
}
fn label(ui: &AppWindow, name: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, name)
        .next()
        .unwrap_or_else(|| panic!("missing label {name}"))
}

pub(crate) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let downloads = Rc::new(RefCell::new(Vec::new()));
    ui.on_download_message({
        let downloads = downloads.clone();
        move |chat, message| {
            downloads.borrow_mut().push((chat, message));
        }
    });
    let commands = Rc::new(RefCell::new(Vec::new()));
    crate::presentation_actions::bind(&ui, {
        let commands = commands.clone();
        move |c| {
            commands.borrow_mut().push(c);
            true
        }
    });
    let mut demo = fixtures::demo();
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for (width, height) in [(1200., 900.), (800., 600.), (520., 480.), (520., 360.)] {
            ui.window().set_size(slint::LogicalSize::new(width, height));
            for chat in [1, 9, 10] {
                demo.select(chat);
                let mut view = demo.conversation();
                view.messages = Arc::from([view.messages[0].clone()]);
                for outgoing in [false, true] {
                    let id = view.messages[0].id;
                    Arc::make_mut(&mut view.messages)[0].outgoing = outgoing;
                    presentation::conversation(&ui, &view, true);
                    ui.show().unwrap();
                    ui.invoke_layout_timeline();
                    // A tall photo may start above the viewport when following
                    // the bottom. Scroll to its top to expose its corner.
                    ui.set_timeline_y(0.);
                    ui.invoke_layout_timeline();
                    let arrow = element(&ui, "MessageBubble::menu-arrow");
                    let bubble = element(&ui, "MessageBubble::bubble");
                    let a = arrow.absolute_position();
                    let b = bubble.absolute_position();
                    assert!(a.x >= b.x && a.x + arrow.size().width <= b.x + bubble.size().width);
                    assert!(
                        a.y >= b.y && a.y + arrow.size().height <= b.y + bubble.size().height,
                        "menu target outside bubble: {width}x{height}, chat {chat}, outgoing {outgoing}, arrow {a:?} {:?}, bubble {b:?} {:?}",
                        arrow.size(),
                        bubble.size()
                    );
                    let geometry = (ui.get_timeline_height(), bubble.size());
                    let model = ui.get_timeline_rows();
                    click(&ui, center(&arrow));
                    assert_eq!(
                        ui.get_message_dialog(),
                        3,
                        "chevron must open and retain menu"
                    );
                    assert!(commands.borrow().is_empty(), "opening the menu is inert");
                    assert_eq!(
                        ElementHandle::find_by_element_type_name(&ui, "MessageMenu").count(),
                        1
                    );
                    for name in ["MessageMenu::strip", "MessageMenu::panel"] {
                        let e = element(&ui, name);
                        let p = e.absolute_position();
                        let s = e.size();
                        assert!(p.x >= 7.5 && p.y >= 7.5, "{name}: {p:?}");
                        assert!(
                            p.x + s.width <= width - 7.5 && p.y + s.height <= height - 7.5,
                            "{name}: {p:?}, {s:?}, {width}x{height}"
                        );
                    }
                    let strip = element(&ui, "MessageMenu::strip");
                    let panel = element(&ui, "MessageMenu::panel");
                    assert!(
                        (center(&strip).x - center(&panel).x).abs() < 0.5,
                        "reaction strip centered over menu"
                    );
                    assert_eq!(
                        geometry,
                        (
                            ui.get_timeline_height(),
                            element(&ui, "MessageBubble::bubble").size()
                        )
                    );
                    assert_eq!(
                        model,
                        ui.get_timeline_rows(),
                        "menu doesn't rebuild timeline"
                    );
                    if a.y >= 58. && a.y + panel.size().height <= height - 8. {
                        assert!(
                            (panel.absolute_position().y - a.y).abs() < 0.5,
                            "menu aligned to chevron"
                        );
                        assert!(
                            strip.absolute_position().y + strip.size().height <= a.y - 5.5,
                            "reaction strip must not cover the chevron"
                        );
                    }
                    let entries = ui.get_message_menu_entries();
                    assert_eq!(entries.iter().any(|e| e.action == 5), chat != 1);
                    assert_eq!(entries.iter().any(|e| e.action == 8), !outgoing);
                    for e in entries.iter().filter(|e| matches!(e.action, 6..=10)) {
                        assert!(!e.enabled);
                    }
                    ui.invoke_choose_message_menu(5);
                    assert!(commands.borrow().is_empty());
                    if chat != 1 {
                        assert_eq!(ui.get_message_dialog(), 0);
                        assert_eq!(downloads.borrow_mut().pop(), Some((chat, id)));
                        ui.invoke_message_action(id, 3, a.x, a.y);
                    } else {
                        assert_eq!(ui.get_message_dialog(), 3);
                    }
                    // Arrow navigation skips disabled rows and reaches the last
                    // enabled entry even when the menu must scroll.
                    key(&ui, slint::platform::Key::End.into());
                    let selection = ui.get_message_menu_selection();
                    assert_eq!(
                        entries.row_data((selection - 7) as usize).unwrap().action,
                        16
                    );
                    key(&ui, slint::platform::Key::Escape.into());
                    assert_eq!(ui.get_message_dialog(), 0);
                    click(&ui, center(&element(&ui, "MessageBubble::menu-arrow")));
                    let dismiss = element(&ui, "MessageMenu::dismiss");
                    assert_eq!(dismiss.size(), slint::LogicalSize::new(width, height));
                    assert_eq!(
                        dismiss.absolute_position(),
                        slint::LogicalPosition::new(0., 0.),
                        "dismiss origin"
                    );
                    click(&ui, slint::LogicalPosition::new(2., 2.));
                    assert_eq!(
                        ui.get_message_dialog(),
                        0,
                        "outside click dismisses without click-through"
                    );
                    click(&ui, center(&element(&ui, "MessageBubble::menu-arrow")));
                    let panel = element(&ui, "MessageMenu::panel");
                    let p = panel.absolute_position();
                    // Also dismiss in the strip's overhang beside the narrower
                    // menu, not only outside the whole popup bounding box.
                    click(
                        &ui,
                        slint::LogicalPosition::new(p.x + panel.size().width + 10., p.y + 20.),
                    );
                    assert_eq!(ui.get_message_dialog(), 0, "click beside menu");
                }
            }
        }
    }
    // Full multiline Unicode copy; no send/download operation is generated.
    ui.window().set_size(slint::LogicalSize::new(800., 700.));
    demo.select(1);
    let mut view = demo.conversation();
    let mut message = view.messages[0].clone();
    message.text = "Complete caption\nȘtiință 🦀\n第二行".into();
    Arc::make_mut(&mut message.content).body = None;
    let id = message.id;
    view.messages = Arc::from([message]);
    presentation::conversation(&ui, &view, true);
    ui.invoke_layout_timeline();
    click(&ui, center(&element(&ui, "MessageBubble::menu-arrow")));
    click(&ui, center(&label(&ui, "Copy")));
    assert_eq!(ui.get_transcript(), "Complete caption\nȘtiință 🦀\n第二行");
    assert_eq!(ui.get_message_dialog(), 0);
    assert!(commands.borrow().is_empty());
    // Quick reaction uses the captured source and the existing durable command.
    ui.invoke_message_action(id, 3, 790., 650.);
    click(&ui, center(&label(&ui, "React with 👍")));
    assert!(
        matches!(commands.borrow_mut().pop().unwrap(),crate::message_actions::Command::React{message,emoji,..} if message==id && emoji=="👍")
    );
    ui.set_action_pending(false);
    ui.invoke_close_message_actions();
    // A revocation while open cannot act on the stale entry.
    ui.invoke_message_action(id, 3, 790., 650.);
    Arc::make_mut(&mut Arc::make_mut(&mut view.messages)[0].content).unavailable = true;
    presentation::conversation(&ui, &view, false);
    ui.invoke_choose_message_menu(1);
    ui.invoke_react_message("❤️".into());
    assert!(commands.borrow().is_empty());
    ui.invoke_close_message_actions();
    // Changing conversations closes the overlay before another input arrives.
    Arc::make_mut(&mut Arc::make_mut(&mut view.messages)[0].content).unavailable = false;
    presentation::conversation(&ui, &view, false);
    ui.invoke_message_action(id, 3, 790., 650.);
    ui.set_selected_id(2);
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_message_dialog(), 0);
    ui.invoke_choose_message_menu(1);
    assert!(commands.borrow().is_empty());
    // Offline copy and browsing reactions remain local; quick reactions and
    // forwarding cannot enqueue network work.
    presentation::conversation(&ui, &view, true);
    ui.set_demo_mode(false);
    ui.set_live_chat_view(true);
    ui.set_live_shown_revision(ui.get_live_revision());
    ui.set_live_can_send(false);
    ui.set_connection_state(0);
    slint::platform::update_timers_and_animations();
    ui.invoke_message_action(id, 3, 400., 300.);
    assert_eq!(ui.get_message_dialog(), 3);
    assert!(
        ui.get_message_menu_entries()
            .iter()
            .any(|e| e.action == 4 && e.enabled)
    );
    assert!(
        ui.get_message_menu_entries()
            .iter()
            .any(|e| e.action == 2 && !e.enabled)
    );
    ui.invoke_react_message("👍".into());
    assert!(commands.borrow().is_empty());
    click(&ui, center(&label(&ui, "More reactions")));
    assert_eq!(ui.get_message_dialog(), 1);
    click(&ui, slint::LogicalPosition::new(2., 2.));
    assert_eq!(ui.get_message_dialog(), 0);
    ui.set_demo_mode(true);

    // Placeholder documents get Download without acquiring a decoder/player
    // or pretending that the placeholder is text suitable for Copy.
    Arc::make_mut(&mut Arc::make_mut(&mut view.messages)[0].content).has_attachment = true;
    presentation::conversation(&ui, &view, true);
    ui.invoke_message_action(id, 3, 400., 300.);
    assert!(
        ui.get_message_menu_entries()
            .iter()
            .any(|e| e.action == 5 && e.enabled)
    );
    assert!(!ui.get_message_menu_entries().iter().any(|e| e.action == 4));
    ui.invoke_close_message_actions();

    demo.select(11);
    let documents = demo.conversation();
    let mut captioned = documents.clone();
    captioned.messages = vec![documents.messages[2].clone()].into();
    let document_id = captioned.messages[0].id;
    presentation::conversation(&ui, &captioned, true);
    ui.invoke_message_action(document_id, 3, 400., 300.);
    assert!(
        ui.get_message_menu_entries()
            .iter()
            .any(|e| e.action == 5 && e.enabled)
    );
    assert!(
        ui.get_message_menu_entries()
            .iter()
            .any(|e| e.action == 4 && e.enabled && e.label == "Copy caption")
    );
    assert_eq!(
        presentation::timeline::action(&ui, document_id, -6),
        crate::message_content::Target::Copy("The worksheet for our next meeting.".into())
    );
    ui.invoke_close_message_actions();

    demo.select(6);
    let mut many = demo.conversation();
    many.messages = demo.chats[5].messages.clone().into();
    presentation::conversation(&ui, &many, true);
    ui.invoke_layout_timeline();
    assert!(ui.get_timeline_rows().row_count() < 32);
    assert!(ElementHandle::find_by_element_type_name(&ui, "MessageMenuArrow").count() < 32);
    assert_eq!(
        ElementHandle::find_by_element_type_name(&ui, "MenuControl").count(),
        0
    );

    // A full background queue reports a failure without a stuck modal.
    crate::presentation_actions::bind(&ui, |_| false);
    let source = ui.get_timeline_rows().row_data(0).unwrap().message.id;
    ui.invoke_message_action(source, 3, 400., 300.);
    ui.invoke_react_message("👍".into());
    assert!(!ui.get_action_pending());
    assert_eq!(ui.get_message_dialog(), 0);
    assert!(ui.get_notice().contains("busy"));
    ui.hide().unwrap();
}
