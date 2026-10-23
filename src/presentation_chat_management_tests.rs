use crate::{AppWindow, fixtures, live_presentation, messaging, presentation};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model};

pub fn check_ui() {
    let ui = AppWindow::new().unwrap();
    let (commands, mut rx) = tokio::sync::mpsc::channel(16);
    live_presentation::bind_messages(&ui, commands);
    ui.set_demo_mode(false);
    ui.set_connection_state(5);
    ui.set_live_chat_view(true);
    let mut rows = fixtures::demo().sidebar();
    rows[0].muted = true;
    rows[0].archived = true;
    rows[0].unread = 120;
    rows[0].call_kind = 1;
    presentation::sidebar(&ui, rows);
    let row = ui.get_chats().row_data(0).unwrap();
    assert!(row.muted && row.archived);
    ui.show().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let first = ElementHandle::find_by_element_id(&ui, "AppWindow::chat-row")
        .next()
        .unwrap();
    let position = first.absolute_position();
    let position = slint::LogicalPosition::new(position.x + 100., position.y + 30.);
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Right,
        });
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position,
            button: slint::platform::PointerEventButton::Right,
        });
    slint::platform::update_timers_and_animations();
    assert!(
        ElementHandle::find_by_element_id(&ui, "ChatMenu::panel")
            .next()
            .is_some()
    );
    assert!(
        rx.try_recv().is_err(),
        "right-click opens options without selecting or modifying a chat"
    );
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::KeyReleased {
            text: slint::platform::Key::Escape.into(),
        });
    for (width, height) in [(1200., 800.), (520., 480.)] {
        ui.window().set_size(slint::LogicalSize::new(width, height));
        let first = ElementHandle::find_by_element_id(&ui, "AppWindow::chat-row")
            .next()
            .unwrap();
        let p = first.absolute_position();
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(p.x + 100., p.y + 30.),
            });
        slint::platform::update_timers_and_animations();
        let child = |id| {
            first
                .query_descendants()
                .match_id(id)
                .find_first()
                .unwrap_or_else(|| panic!("missing {id} at {width}px"))
        };
        let preview = child("AppWindow::chat-preview");
        let mute = child("AppWindow::chat-mute");
        let options = child("AppWindow::chat-options");
        let badge = child("AppWindow::badge");
        for pair in [&preview, &mute, &options, &badge].windows(2) {
            assert!(
                pair[0].absolute_position().x + pair[0].size().width
                    <= pair[1].absolute_position().x,
                "preview, mute, options and unread count must not overlap at {width}px"
            );
        }
        assert!(
            badge.absolute_position().x + badge.size().width
                <= first.absolute_position().x + first.size().width
        );
        ui.invoke_open_chat_menu(row.clone(), width - 1., height - 1.);
        slint::platform::update_timers_and_animations();
        let menu = ElementHandle::find_by_element_id(&ui, "ChatMenu::panel")
            .next()
            .unwrap();
        let rect = menu.absolute_position();
        let size = menu.size();
        assert!(rect.x >= 0. && rect.y >= 0.);
        assert!(rect.x + size.width <= width && rect.y + size.height <= height);
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased {
                text: slint::platform::Key::Escape.into(),
            });
    }
    ui.invoke_open_chat_menu(row.clone(), 10., 10.);
    slint::platform::update_timers_and_animations();
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Return.into(),
        });
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::KeyReleased {
            text: slint::platform::Key::Return.into(),
        });
    assert!(
        matches!(rx.try_recv().unwrap(),messaging::Command::ManageChat {chat,action:crate::chat_management::Action::Unmute} if chat==row.id)
    );
    assert!(ui.get_chat_management_busy());
    ui.invoke_manage_chat(row.id, 6);
    assert!(rx.try_recv().is_err(), "one action at a time");
    ui.set_chat_management_busy(false);
    let mut unmuted = row.clone();
    unmuted.muted = false;
    ui.invoke_open_chat_menu(unmuted, 10., 10.);
    slint::platform::update_timers_and_animations();
    crate::presentation_calling::workflow_tests::click_button(&ui, "Mute always");
    assert!(
        matches!(rx.try_recv().unwrap(),messaging::Command::ManageChat {chat,action:crate::chat_management::Action::MuteAlways} if chat==row.id),
        "clicking Mute always must submit the captured chat and action"
    );
    assert!(ui.get_chat_management_busy());
    assert_eq!(ui.get_notice(), "Updating chat…");
    ui.set_chat_management_busy(false);
    ui.set_draft("Keep while changing folder".into());
    ui.invoke_change_chat_folder(true);
    assert!(
        matches!(rx.try_recv().unwrap(),messaging::Command::Folder {archived:true,draft,..} if draft=="Keep while changing folder")
    );
    ui.set_selected_id(row.id);
    ui.set_photo_viewer_id(1);
    ui.set_message_dialog(3);
    ui.set_transcript("Synthetic private transcript".into());
    live_presentation::publish_messages(
        &ui,
        &messaging::Snapshot {
            revision: ui.get_live_revision(),
            view: Some(std::sync::Arc::new(crate::archive::View::default())),
            ..messaging::Snapshot::empty()
        },
    );
    assert_eq!(ui.get_selected_id(), 0);
    assert_eq!(ui.get_photo_viewer_id(), 0);
    assert_eq!(ui.get_message_dialog(), 0);
    assert!(ui.get_transcript().is_empty());
    live_presentation::publish_messages(
        &ui,
        &messaging::Snapshot {
            revision: ui.get_live_revision(),
            notice: "Synthetic unconfirmed chat change".into(),
            view: Some(std::sync::Arc::new(crate::archive::View {
                history_failed: true,
                ..Default::default()
            })),
            ..messaging::Snapshot::empty()
        },
    );
    assert_eq!(
        ui.get_notice(),
        "Synthetic unconfirmed chat change",
        "an old history warning must not hide an action result"
    );
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    presentation::sidebar(&ui, {
        let mut rows = fixtures::demo().sidebar();
        rows[0].muted = true;
        rows[0].unread = 120;
        rows
    });
    ui.set_sidebar_hidden(true);
    slint::platform::update_timers_and_animations();
    let mute = ElementHandle::find_by_element_id(&ui, "CompactChat::mute-marker")
        .next()
        .unwrap();
    let badge = ElementHandle::find_by_element_id(&ui, "CompactChat::badge")
        .next()
        .unwrap();
    let sidebar = ElementHandle::find_by_element_id(&ui, "AppWindow::compact-sidebar")
        .next()
        .unwrap();
    assert!(mute.absolute_position().y >= badge.absolute_position().y + badge.size().height);
    assert!(mute.absolute_position().x >= sidebar.absolute_position().x);
    assert!(
        mute.absolute_position().x + mute.size().width
            <= sidebar.absolute_position().x + sidebar.size().width
    );
    ui.hide().unwrap();
}
