//! Real callback boundaries with delayed/failing archive replies; no account traffic.
use crate::{
    AppWindow, Theme, fixtures,
    message_actions::{Command, OperationReply},
    presentation, presentation_actions,
    ui::PollDraftOption,
};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc, sync::Arc};
fn ack(ui: &AppWindow, kind: u8, saved: bool) {
    presentation_actions::publish(
        ui,
        Some(&OperationReply {
            request: ui.get_action_request(),
            kind,
            saved,
            error: if saved {
                String::new()
            } else {
                "Synthetic archive failure".into()
            },
            ..Default::default()
        }),
        &Arc::from([]),
    );
}
pub fn check_ui() {
    check_live_refresh_while_typing();
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let mut demo = fixtures::demo();
    let c = demo.chats.iter_mut().find(|c| c.id == 7).unwrap();
    fixtures::fill_polls(&mut c.messages);
    demo.select(7);
    presentation::conversation(&ui, &demo.conversation(), true);
    ui.set_draft("Keep composer 👩🏽‍💻".into());
    let commands = Rc::new(RefCell::new(Vec::new()));
    presentation_actions::bind(&ui, {
        let commands = commands.clone();
        move |c| {
            commands.borrow_mut().push(c);
            true
        }
    });
    for (w, h, dark) in [(1200., 900., false), (760., 600., true)] {
        ui.window().set_size(slint::LogicalSize::new(w, h));
        ui.global::<Theme>().set_dark(dark);
        ui.invoke_layout_timeline();
        let mut bubbles = ElementHandle::find_by_element_type_name(&ui, "MessageBubble")
            .filter(|e| e.id().as_deref() != Some("AppWindow::measure-bubble"))
            .collect::<Vec<_>>();
        bubbles.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
        assert!(!bubbles.is_empty());
        for pair in bubbles.windows(2) {
            assert!(
                pair[0].absolute_position().y + pair[0].size().height
                    <= pair[1].absolute_position().y + 0.2,
                "Poll cards overlap"
            );
        }
        ui.invoke_open_create_poll();
        assert_eq!(ui.get_message_dialog(), 10);
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Poll question")
                .next()
                .is_some()
        );
        ui.invoke_send_poll();
        assert!(!ui.get_action_pending());
        assert!(!ui.get_action_error().is_empty());
        ui.set_poll_question("A multiline question\nwith Unicode 👩🏽‍💻".into());
        ui.set_poll_draft_options(ModelRc::new(VecModel::from(vec![
            PollDraftOption { text: "Yes".into() },
            PollDraftOption { text: "No".into() },
        ])));
        ui.invoke_send_poll();
        ui.invoke_send_poll();
        assert!(ui.get_action_pending());
        assert!(matches!(
            commands.borrow().last().unwrap(),
            Command::CreatePoll { .. }
        ));
        let count = commands.borrow().len();
        ack(&ui, 12, false);
        assert_eq!(ui.get_message_dialog(), 10);
        assert_eq!(ui.get_poll_draft_options().row_count(), 2);
        assert!(!ui.get_action_pending());
        ui.invoke_send_poll();
        assert_eq!(commands.borrow().len(), count + 1);
        ack(&ui, 12, true);
        assert_eq!(ui.get_message_dialog(), 0);
        assert_eq!(ui.get_draft(), "Keep composer 👩🏽‍💻");
    }
    ui.invoke_vote_poll(57000, 1);
    ui.invoke_vote_poll(57000, 1);
    let n = commands.borrow().len();
    assert!(matches!(
        commands.borrow().last().unwrap(),
        Command::VotePoll { option: 1, .. }
    ));
    ack(&ui, 13, false);
    ui.invoke_close_error();
    ui.invoke_vote_poll(57000, 1);
    assert_eq!(commands.borrow().len(), n + 1);
    ack(&ui, 13, true);
    ui.invoke_show_poll_results(57000, 0, 100);
    assert_eq!(ui.get_message_dialog(), 11);
    assert!(
        ElementHandle::find_by_element_type_name(&ui, "MessageDetails")
            .next()
            .is_none()
    );
    assert!(ui.get_action_pending());
    let result = OperationReply {
        request: ui.get_action_request(),
        kind: 14,
        saved: true,
        poll_results: Some(Arc::new(crate::polls::Results {
            message: 57000,
            question: "When?".into(),
            options: vec!["Saturday".into(), "Sunday".into()],
            counts: vec![2, 1],
            voters: vec![crate::polls::Voter {
                jid: "#self".into(),
                name: "You".into(),
                ..Default::default()
            }],
            ..Default::default()
        })),
        ..Default::default()
    };
    presentation_actions::publish(&ui, Some(&result), &Arc::from([]));
    assert_eq!(ui.get_poll_result_voters().row_count(), 1);
    assert!(!ui.get_action_pending());
    ui.invoke_close_message_actions();
    presentation_actions::publish(&ui, Some(&result), &Arc::from([]));
    assert_eq!(ui.get_message_dialog(), 0);
    ui.invoke_show_poll_results(57000, 0, 100);
    assert!(ui.get_action_pending());
    let cancelled = OperationReply {
        request: ui.get_action_request(),
        ..result.clone()
    };
    ui.invoke_close_message_actions();
    assert!(
        !ui.get_action_pending(),
        "Closing a pending read-only query must release the gate"
    );
    presentation_actions::publish(&ui, Some(&cancelled), &Arc::from([]));
    assert_eq!(
        ui.get_message_dialog(),
        0,
        "Late results must not reopen a closed dialog"
    );
    // A rejected queue retains the creation form and its inputs.
    let other = AppWindow::new().unwrap();
    other.set_demo_mode(true);
    presentation::conversation(&other, &demo.conversation(), true);
    presentation_actions::bind(&other, |_| false);
    other.invoke_open_create_poll();
    other.set_poll_question("Question".into());
    other.set_poll_draft_options(ModelRc::new(VecModel::from(vec![
        PollDraftOption { text: "A".into() },
        PollDraftOption { text: "B".into() },
    ])));
    other.invoke_send_poll();
    assert!(!other.get_action_pending());
    assert_eq!(other.get_message_dialog(), 10);
    assert!(!other.get_action_error().is_empty());
}

fn check_live_refresh_while_typing() {
    use crate::{connection, live_presentation, messaging};
    use slint::platform::{PointerEventButton, WindowEvent};
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    ui.set_show_conversation(true);
    let (commands, mut received) = tokio::sync::mpsc::channel(256);
    live_presentation::bind_messages(&ui, commands);
    live_presentation::publish(
        &ui,
        &connection::Snapshot::new(connection::Phase::Connected),
    );
    let mut demo = fixtures::demo();
    demo.select(1);
    let refresh = |demo: &crate::domain::DemoState| {
        live_presentation::publish_messages(
            &ui,
            &messaging::Snapshot {
                revision: ui.get_live_revision(),
                view: Some(Arc::new(crate::archive::View {
                    chats: demo.sidebar().into(),
                    conversation: Some(demo.conversation()),
                    can_send: true,
                    ..Default::default()
                })),
                ..messaging::Snapshot::empty()
            },
        );
    };
    refresh(&demo);
    ui.show().unwrap();
    ui.invoke_open_create_poll();
    assert_eq!(ui.get_message_dialog(), 10);
    let input = ElementHandle::find_by_element_id(&ui, "CreatePollDialog::question")
        .next()
        .unwrap();
    let pos = input.absolute_position();
    let position = slint::LogicalPosition::new(pos.x + 10., pos.y + 10.);
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    let mut expected = String::new();
    for text in ["Lunch", " tomorrow", " 👩🏽‍💻", " at noon?"] {
        for ch in text.chars() {
            let text: slint::SharedString = ch.into();
            ui.window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            ui.window()
                .dispatch_event(WindowEvent::KeyReleased { text });
        }
        expected.push_str(text);
        assert_eq!(ui.get_poll_question(), expected);
        refresh(&demo);
        slint::platform::update_timers_and_animations();
        assert_eq!(
            ui.get_message_dialog(),
            10,
            "A same-chat archive refresh must not close poll creation"
        );
        assert_eq!(
            ui.get_poll_question(),
            expected,
            "Background refresh must retain the input and keyboard focus"
        );
    }
    let option = ElementHandle::find_by_accessible_label(&ui, "Poll option 1")
        .next()
        .unwrap();
    let pos = option.absolute_position();
    let position = slint::LogicalPosition::new(pos.x + 10., pos.y + 10.);
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    for text in ["Picnic", " 🧺"] {
        for ch in text.chars() {
            let text: slint::SharedString = ch.into();
            ui.window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            ui.window()
                .dispatch_event(WindowEvent::KeyReleased { text });
        }
        refresh(&demo);
        assert_eq!(ui.get_message_dialog(), 10);
    }
    assert_eq!(
        ui.get_poll_draft_options().row_data(0).unwrap().text,
        "Picnic 🧺"
    );
    ui.set_poll_multiple(false);
    let mut incoming = demo.chats[0].messages[0].clone();
    incoming.id = 99001;
    incoming.text = "Synthetic background message".into();
    demo.chats[0].messages.push(incoming);
    refresh(&demo);
    assert_eq!(
        ui.get_message_dialog(),
        10,
        "An incoming message must not discard the poll form"
    );
    assert_eq!(ui.get_poll_question(), expected);
    assert_eq!(
        ui.get_poll_draft_options().row_data(0).unwrap().text,
        "Picnic 🧺"
    );
    assert!(!ui.get_poll_multiple());
    while let Ok(command) = received.try_recv() {
        assert!(
            !matches!(
                command,
                messaging::Command::Action(Command::CreatePoll { .. })
            ),
            "Typing must not submit the poll"
        );
    }
    ui.invoke_close_message_actions();
    assert_eq!(ui.get_message_dialog(), 0);
    ui.invoke_open_create_poll();
    demo.select(2);
    refresh(&demo);
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_message_dialog(),
        0,
        "Actually changing conversations must still close the form"
    );
    ui.hide().unwrap();
}
