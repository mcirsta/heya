//! Synthetic input and state transitions; never opens an account or sends to WhatsApp.
use crate::{
    AppWindow, Theme, fixtures,
    message_actions::{Command, OperationReply},
    presentation,
};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model, platform::WindowEvent};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};
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
fn right(ui: &AppWindow, p: slint::LogicalPosition) {
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: p });
    let mut target = None;
    assert!(crate::presentation_scroll::context_pointer(
        ui,
        p,
        true,
        &mut target
    ));
    assert_eq!(ui.get_message_dialog(), 0, "menu opens on release");
    assert!(crate::presentation_scroll::context_pointer(
        ui,
        p,
        false,
        &mut target
    ));
    slint::platform::update_timers_and_animations();
}
fn key(ui: &AppWindow, key: slint::SharedString) {
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key });
    slint::platform::update_timers_and_animations();
}
fn finish(ui: &AppWindow, kind: u8) {
    crate::presentation_actions::publish(
        ui,
        Some(&OperationReply {
            request: ui.get_action_request(),
            kind,
            saved: true,
            ..Default::default()
        }),
        &Arc::from([]),
    );
}
pub(crate) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let commands = Rc::new(RefCell::new(Vec::new()));
    crate::presentation_actions::bind(&ui, {
        let c = commands.clone();
        move |command| {
            c.borrow_mut().push(command);
            true
        }
    });
    let activations = Rc::new(Cell::new(0));
    ui.on_activate_message({
        let a = activations.clone();
        move |_, _| a.set(a.get() + 1)
    });
    ui.on_activate_photo({
        let a = activations.clone();
        move |_| a.set(a.get() + 1)
    });
    ui.on_audio_control({
        let a = activations.clone();
        move |_, _, _| a.set(a.get() + 1)
    });
    let mut demo = fixtures::demo();
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for (w, h) in [(1200., 900.), (520., 480.)] {
            ui.window().set_size(slint::LogicalSize::new(w, h));
            for chat in [1, 7, 9, 10, 11] {
                demo.select(chat);
                let mut view = demo.conversation();
                let mut m = view.messages[0].clone();
                m.outgoing = true;
                Arc::make_mut(&mut m.content).metadata.delivery = "accepted".into();
                view.messages = Arc::from([m]);
                presentation::conversation(&ui, &view, true);
                ui.show().unwrap();
                ui.invoke_layout_timeline();
                ui.set_timeline_y(0.);
                ui.invoke_layout_timeline();
                let id = match chat {
                    7 => "MessageBubble::preview",
                    9 => "MessageBubble::photo",
                    10 => "AudioPlayer::play",
                    11 => "MessageBubble::document",
                    _ => "MessageBubble::body",
                };
                let hit = center(&element(&ui, id));
                let before = activations.get();
                let model = ui.get_timeline_rows();
                let geometry = ui.get_timeline_height();
                right(&ui, hit);
                assert_eq!(ui.get_message_dialog(), 3, "right click on {id}");
                assert!(ui.get_action_context_menu());
                assert_eq!(
                    activations.get(),
                    before,
                    "right click cannot activate {id}"
                );
                assert!(commands.borrow().is_empty());
                for id in ["MessageMenu::panel", "MessageMenu::strip"] {
                    let e = element(&ui, id);
                    let p = e.absolute_position();
                    let s = e.size();
                    assert!(
                        p.x >= 8.
                            && p.y >= 8.
                            && p.x + s.width <= w - 7.5
                            && p.y + s.height <= h - 7.5,
                        "popup containment {id}: {p:?} {s:?}"
                    );
                }
                if chat == 1 {
                    assert_eq!(
                        ui.get_message_menu_entries()
                            .iter()
                            .map(|e| e.action)
                            .collect::<Vec<_>>(),
                        [1, 2, 11, 4, 12, 13, 14, 15, 16]
                    );
                }
                assert_eq!(geometry, ui.get_timeline_height());
                assert_eq!(model, ui.get_timeline_rows());
                key(&ui, slint::platform::Key::Escape.into());
                assert_eq!(ui.get_message_dialog(), 0);
                // ZapFast also accepts the otherwise empty row beside a bubble.
                let b = element(&ui, "MessageBubble::bubble");
                let p = center(&b);
                right(
                    &ui,
                    slint::LogicalPosition::new(
                        element(&ui, "AppWindow::timeline").absolute_position().x + 10.,
                        p.y,
                    ),
                );
                assert_eq!(ui.get_message_dialog(), 3);
                ui.invoke_close_message_actions();
            }
        }
    }
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    demo.select(1);
    let mut view = demo.conversation();
    let mut m = view.messages[0].clone();
    m.outgoing = true;
    Arc::make_mut(&mut m.content).metadata.delivery = "accepted".into();
    m.text = "Full original\nȘtiință 🦀 第二行".into();
    Arc::make_mut(&mut m.content).body = None;
    let id = m.id;
    view.messages = Arc::from([m]);
    presentation::conversation(&ui, &view, true);
    ui.invoke_layout_timeline();
    ui.set_draft("Separate unsent draft".into());
    ui.invoke_message_action(id, 17, 900., 200.);
    ui.invoke_choose_message_menu(16);
    assert_eq!(ui.get_transcript(), format!("DEMO-{id}"));
    ui.invoke_message_action(id, 17, 900., 200.);
    ui.invoke_choose_message_menu(12);
    assert_eq!(ui.get_message_dialog(), 5);
    assert_eq!(ui.get_action_edit_text(), view.messages[0].text);
    ui.set_action_edit_text("Edited\nUnicode 👩🏽‍💻".into());
    ui.invoke_confirm_message_control();
    assert!(
        matches!(commands.borrow_mut().pop().unwrap(),Command::Edit{message,expected,text,..} if message==id && expected==view.messages[0].text && text=="Edited\nUnicode 👩🏽‍💻")
    );
    assert_eq!(ui.get_draft(), "Separate unsent draft");
    finish(&ui, 4);
    assert_eq!(ui.get_message_dialog(), 0);
    ui.invoke_message_action(id, 17, 900., 200.);
    ui.invoke_choose_message_menu(13);
    assert_eq!(ui.get_message_dialog(), 6);
    key(&ui, slint::platform::Key::Return.into());
    assert!(
        commands.borrow().is_empty(),
        "Enter must not confirm deletion"
    );
    // Default focus may activate Cancel. Enter must never enqueue deletion.
    if ui.get_message_dialog() == 0 {
        ui.invoke_message_action(id, 17, 900., 200.);
        ui.invoke_choose_message_menu(13);
    }
    assert!(!ui.get_action_pending());
    assert_eq!(ui.get_action_chat(), ui.get_selected_id());
    ui.invoke_confirm_message_control();
    assert!(
        matches!(commands.borrow_mut().pop().unwrap(),Command::Revoke{message,..} if message==id)
    );
    finish(&ui, 5);
    ui.invoke_message_action(id, 17, 900., 200.);
    ui.invoke_choose_message_menu(11);
    assert!(ui.get_selection_active());
    assert_eq!(ui.get_selection_count(), 1);
    let model = ui.get_timeline_rows();
    let height = ui.get_timeline_height();
    ui.invoke_selection_control(1);
    assert_eq!(ui.get_transcript(), view.messages[0].text);
    ui.invoke_select_message(id);
    assert_eq!(ui.get_selection_count(), 0);
    assert!(ui.get_selection_active());
    ui.invoke_select_message(id);
    assert_eq!(model, ui.get_timeline_rows());
    assert_eq!(height, ui.get_timeline_height());
    assert_eq!(ui.get_draft(), "Separate unsent draft");
    ui.invoke_selection_control(2);
    assert!(matches!(
        commands.borrow_mut().pop().unwrap(),
        Command::Destinations { .. }
    ));
    crate::presentation_actions::publish(
        &ui,
        Some(&OperationReply {
            request: ui.get_action_request(),
            kind: 3,
            saved: true,
            ..Default::default()
        }),
        &Arc::from([crate::message_actions::Destination {
            id: 2,
            title: "Destination".into(),
        }]),
    );
    ui.invoke_forward_message(2);
    assert!(
        matches!(commands.borrow_mut().pop().unwrap(),Command::ForwardMany{messages,destination:2,..} if messages==[id])
    );
    finish(&ui, 2);
    ui.invoke_message_action(id, 11, 0., 0.);
    ui.invoke_selection_control(3);
    assert_eq!(ui.get_message_dialog(), 7);
    key(&ui, slint::platform::Key::Return.into());
    assert!(commands.borrow().is_empty());
    if ui.get_message_dialog() == 0 {
        ui.invoke_selection_control(3);
    }
    ui.invoke_confirm_message_control();
    assert!(
        matches!(commands.borrow_mut().pop().unwrap(),Command::Hide{messages,..} if messages==[id])
    );
    finish(&ui, 6);
    assert!(!ui.get_selection_active());
    ui.invoke_message_action(id, 17, 900., 200.);
    ui.invoke_choose_message_menu(15);
    assert_eq!(ui.get_message_dialog(), 8);
    assert!(
        matches!(commands.borrow_mut().pop().unwrap(),Command::Info{message,..} if message==id)
    );
    let mut info = crate::message_actions::Info {
        message: id,
        wire: "EXACT-WIRE-ID".into(),
        sent: "Synthetic time".into(),
        status: "Sent".into(),
        rows: Vec::new(),
    };
    let result = |info| OperationReply {
        request: ui.get_action_request(),
        kind: 7,
        saved: true,
        info: Some(info),
        ..Default::default()
    };
    crate::presentation_actions::publish(&ui, Some(&result(info.clone())), &Arc::from([]));
    assert_eq!(ui.get_info_wire(), "EXACT-WIRE-ID");
    assert!(!ui.get_action_pending());
    info.rows.push(crate::message_actions::InfoRow {
        name: "Synthetic recipient".into(),
        detail: "Read · supplied time".into(),
    });
    crate::presentation_actions::publish(&ui, Some(&result(info)), &Arc::from([]));
    assert_eq!(ui.get_info_receipts().row_count(), 1);
    ui.invoke_close_message_actions();
    slint::platform::update_timers_and_animations();
    assert!(
        commands
            .borrow_mut()
            .drain(..)
            .any(|c| matches!(c, Command::CloseInfo))
    );
    // Capability changes are checked again on activation, including expiry.
    ui.invoke_message_action(id, 17, 900., 200.);
    Arc::make_mut(&mut Arc::make_mut(&mut view.messages)[0].content)
        .metadata
        .stamp = chrono::Utc::now().timestamp() - crate::message_actions::EDIT_WINDOW - 5;
    presentation::conversation(&ui, &view, false);
    ui.invoke_choose_message_menu(12);
    assert!(commands.borrow().is_empty());
    assert_eq!(ui.get_message_dialog(), 3);
    ui.invoke_close_message_actions();
    ui.invoke_prepare_quick_reactions("🦀".into());
    assert_eq!(
        ui.get_menu_reactions()
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>(),
        ["👍", "❤️", "😂", "😮", "😢", "🙏", "🦀", "+"]
    );
    ui.invoke_remember_reaction("😉".into());
    ui.invoke_prepare_quick_reactions("".into());
    assert_eq!(ui.get_menu_reactions().row_data(0).unwrap(), "😉");
    // Selecting uses the existing virtual rows, even in a 10,000-message source.
    demo.select(6);
    let mut many = demo.conversation();
    many.messages = demo.chats[5].messages.clone().into();
    presentation::conversation(&ui, &many, true);
    ui.invoke_layout_timeline();
    let rows = ui.get_timeline_rows();
    let source = rows
        .iter()
        .find(|r| r.message.actionable)
        .unwrap()
        .message
        .id;
    let height = ui.get_timeline_height();
    ui.invoke_message_action(source, 11, 0., 0.);
    assert!(ui.get_selection_active());
    assert_eq!(rows, ui.get_timeline_rows());
    assert!(rows.row_count() < 32);
    assert_eq!(height, ui.get_timeline_height());
    assert_eq!(
        ElementHandle::find_by_element_type_name(&ui, "MenuControl").count(),
        0
    );
    ui.invoke_selection_control(0);
    ui.hide().unwrap();
}
