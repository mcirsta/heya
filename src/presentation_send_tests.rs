//! Real composer key dispatch with a delayed, synthetic durable-save reply.
use crate::{AppWindow, connection, fixtures, live_presentation, messaging};
use i_slint_backend_testing::ElementHandle;
use slint::{
    ComponentHandle,
    platform::{Key, WindowEvent},
};

fn key(ui: &AppWindow, text: impl Into<slint::SharedString>) {
    let text = text.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text });
}

pub fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let (commands, mut rx) = tokio::sync::mpsc::channel(256);
    live_presentation::bind_messages(&ui, commands);
    let state = fixtures::demo();
    let snapshot = messaging::Snapshot {
        view: Some(std::sync::Arc::new(crate::archive::View {
            unread_chats: 0,
            archived_count: 0,
            archived_view: false,
            chats: state.sidebar().into(),
            conversation: Some(state.conversation()),
            can_send: true,
            history_failed: false,
            phone_more: false,
        })),
        ..messaging::Snapshot::empty()
    };
    ui.set_demo_mode(false);
    ui.set_show_conversation(true);
    live_presentation::publish(
        &ui,
        &connection::Snapshot::new(connection::Phase::Connected),
    );
    live_presentation::publish_messages(&ui, &snapshot);
    ui.show().unwrap();
    assert!(
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&ui, "Message draft")
            .next()
            .is_some()
    );
    ui.invoke_focus_composer();
    let text = "A long synthetic paragraph 👩🏽‍💻 with diacritics: ăâîșț.\n".repeat(120);
    // Repeat without refocusing the editor: publishing a save must retain its
    // keyboard focus, including at narrow widths and after long wrapped drafts.
    for width in [1200., 620., 520.] {
        ui.window().set_size(slint::LogicalSize::new(width, 800.));
        ui.set_draft(text.clone().into());
        ui.invoke_draft_edited(ui.get_draft());
        rx.try_recv().unwrap();
        let height = ElementHandle::find_by_element_type_name(&ui, "Composer")
            .next()
            .unwrap()
            .size()
            .height;
        key(&ui, Key::Return);
        let messaging::Command::Send {
            chat,
            text: sent,
            request,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("Enter must submit");
        };
        assert_eq!(sent, text);
        assert!(ui.get_send_pending());
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Saving message…")
                .next()
                .is_some()
        );
        assert!(ui.get_notice().is_empty(), "saving must not open a banner");
        assert_eq!(
            ElementHandle::find_by_element_type_name(&ui, "Composer")
                .next()
                .unwrap()
                .size()
                .height,
            height
        );
        key(&ui, Key::Return);
        assert!(
            rx.try_recv().is_err(),
            "pending Enter must not duplicate the send"
        );
        assert_eq!(ui.get_draft(), text);
        // Other input is responsive while the fake archive reply is delayed.
        key(&ui, "x");
        assert!(ui.get_draft().contains('x'));
        let messaging::Command::Draft(_, newer, _) = rx.try_recv().unwrap() else {
            panic!("typing must remain responsive");
        };
        let layout_revision = ui.get_view_revision();
        let reply = messaging::SendReply {
            request,
            chat,
            target: chat,
            text: sent,
            saved: true,
        };
        live_presentation::publish_messages(
            &ui,
            &messaging::Snapshot {
                revision: ui.get_live_revision(),
                reply: Some(reply.clone()),
                ..messaging::Snapshot::empty()
            },
        );
        assert_eq!(
            ui.get_view_revision(),
            layout_revision,
            "a save reply must not rebuild the timeline"
        );
        assert!(
            ui.get_live_can_send(),
            "a reply must keep conversation capabilities"
        );
        assert!(!ui.get_send_pending());
        assert_eq!(
            ui.get_draft(),
            newer,
            "keep edits made during the delayed save"
        );
        // Native key-repeat events can arrive after the first save completes.
        // They must not send a retained/newer draft just because Enter is held.
        let mut event = i_slint_core::input::InternalKeyEvent::default();
        event.key_event.text = Key::Return.into();
        event.key_event.repeat = true;
        ui.window().dispatch_event(WindowEvent::internal(event));
        assert!(
            rx.try_recv().is_err(),
            "holding Enter must not resubmit after completion"
        );
        key(&ui, Key::Return);
        let messaging::Command::Send {
            chat,
            text,
            request,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("the next explicit press must work");
        };
        // A delayed full snapshot for the previous send cannot finish the new one.
        live_presentation::publish_messages(
            &ui,
            &messaging::Snapshot {
                reply: Some(reply),
                ..snapshot.clone()
            },
        );
        assert!(ui.get_send_pending());
        assert_eq!(ui.get_draft(), text);
        live_presentation::publish_messages(
            &ui,
            &messaging::Snapshot {
                reply: Some(messaging::SendReply {
                    request,
                    chat,
                    target: chat,
                    text,
                    saved: true,
                }),
                ..messaging::Snapshot::empty()
            },
        );
        assert!(ui.get_draft().is_empty());
        assert!(!ui.get_send_pending());
        assert!(ui.get_notice().is_empty());
    }
    ui.set_draft("Preserve the failed draft".into());
    key(&ui, Key::Return);
    let messaging::Command::Send {
        chat,
        text,
        request,
        ..
    } = rx.try_recv().unwrap()
    else {
        panic!();
    };
    live_presentation::publish_messages(
        &ui,
        &messaging::Snapshot {
            reply: Some(messaging::SendReply {
                request,
                chat,
                target: chat,
                text,
                saved: false,
            }),
            ..messaging::Snapshot::empty()
        },
    );
    assert!(!ui.get_send_pending());
    assert_eq!(ui.get_draft(), "Preserve the failed draft");
    assert!(ui.get_notice().contains("could not be saved"));
    ui.set_connection_state(6);
    ui.set_notice("".into());
    key(&ui, Key::Return);
    assert!(rx.try_recv().is_err());
    assert!(!ui.get_send_pending());
    assert_eq!(ui.get_draft(), "Preserve the failed draft");
    ui.set_connection_state(5);
    for _ in 0..256 {
        ui.invoke_search_chats("Synthetic queue pressure".into());
    }
    key(&ui, Key::Return);
    assert!(
        !ui.get_send_pending(),
        "a full queue cannot leave a phantom pending send"
    );
    assert!(ui.get_notice().contains("busy"));
    assert_eq!(ui.get_draft(), "Preserve the failed draft");
    while rx.try_recv().is_ok() {}
    key(&ui, Key::Return);
    let messaging::Command::Send {
        chat,
        text,
        request,
        ..
    } = rx.try_recv().unwrap()
    else {
        panic!()
    };
    assert!(ui.get_send_pending(), "retry works once the queue drains");
    ui.invoke_select_chat(2);
    let revision = ui.get_live_shown_revision();
    assert_ne!(revision, ui.get_live_revision());
    live_presentation::publish_messages(
        &ui,
        &messaging::Snapshot {
            revision: ui.get_live_revision(),
            reply: Some(messaging::SendReply {
                chat,
                target: chat,
                text,
                request,
                saved: true,
            }),
            ..messaging::Snapshot::empty()
        },
    );
    assert!(!ui.get_send_pending());
    assert_eq!(
        ui.get_live_shown_revision(),
        revision,
        "a save reply must not unlock unfinished navigation"
    );
    ui.hide().unwrap();
}

/// A PN/LID merge can publish the target conversation before the send reply.
pub fn check_merged_chat_ui() {
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(900., 700.));
    let (commands, mut rx) = tokio::sync::mpsc::channel(256);
    live_presentation::bind_messages(&ui, commands);
    let mut state = fixtures::demo();
    let chats: std::sync::Arc<[_]> = state.sidebar().into();
    let view = |conversation: crate::domain::Conversation, revision| messaging::Snapshot {
        revision,
        view: Some(std::sync::Arc::new(crate::archive::View {
            unread_chats: 0,
            archived_count: 0,
            archived_view: false,
            chats: chats.clone(),
            conversation: Some(conversation),
            can_send: true,
            history_failed: false,
            phone_more: false,
        })),
        ..messaging::Snapshot::empty()
    };
    ui.set_demo_mode(false);
    ui.set_show_conversation(true);
    live_presentation::publish(
        &ui,
        &connection::Snapshot::new(connection::Phase::Connected),
    );
    let original = state.conversation();
    live_presentation::publish_messages(&ui, &view(original.clone(), 0));
    ui.show().unwrap();
    ui.invoke_focus_composer();
    let text = "Synthetic merged send 👩🏽‍💻\năâîșț";
    ui.set_draft(text.into());
    ui.invoke_draft_edited(ui.get_draft());
    rx.try_recv().unwrap();
    key(&ui, Key::Return);
    let messaging::Command::Send { chat, request, .. } = rx.try_recv().unwrap() else {
        panic!("Enter must submit");
    };
    let mut merged = original.clone();
    merged.chat.id = 9_000;
    merged.draft = text.into();
    live_presentation::publish_messages(&ui, &view(merged.clone(), ui.get_live_revision()));
    assert_eq!(ui.get_selected_id(), 9_000);
    assert!(ui.get_send_pending());
    assert_eq!(ui.get_draft(), text);
    let reply = |chat, target| messaging::SendReply {
        request: ui.get_send_request_id(),
        chat,
        target,
        text: ui.get_draft().into(),
        saved: true,
    };
    let merged_reply = messaging::Snapshot {
        reply: Some(reply(chat, 9_000)),
        ..messaging::Snapshot::empty()
    };
    assert_eq!(merged_reply.reply.as_ref().unwrap().request, request);
    live_presentation::publish_messages(&ui, &merged_reply);
    assert!(!ui.get_send_pending());
    assert!(
        ui.get_draft().is_empty(),
        "the merged conversation's composer held the sent text"
    );
    // A reply must still never clear another conversation's composer.
    ui.set_draft("Second synthetic send".into());
    ui.invoke_draft_edited(ui.get_draft());
    rx.try_recv().unwrap();
    key(&ui, Key::Return);
    let messaging::Command::Send { .. } = rx.try_recv().unwrap() else {
        panic!("Enter must submit");
    };
    let late = reply(9_000, 9_000);
    ui.invoke_select_chat(2);
    state.select(2);
    let mut other = state.conversation();
    other.draft = "Other chat draft".into();
    live_presentation::publish_messages(&ui, &view(other, ui.get_live_revision()));
    live_presentation::publish_messages(
        &ui,
        &messaging::Snapshot {
            reply: Some(late),
            ..messaging::Snapshot::empty()
        },
    );
    assert!(!ui.get_send_pending());
    assert_eq!(ui.get_draft(), "Other chat draft");
    ui.hide().unwrap();
}
