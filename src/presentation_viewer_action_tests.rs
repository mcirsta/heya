//! Synthetic viewer actions share the ordinary message workflow and queue.
use crate::{
    AppWindow, MessageRow, fixtures,
    message_actions::{Command, Destination, OperationReply},
    presentation, presentation_actions,
};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

const PHOTO: i32 = 70001;

fn activate(ui: &AppWindow, label: &str) {
    // Background timeline controls can have the same label (notably an idle
    // photo's Download target); the test backend does not filter occlusion.
    let component = if label.starts_with("Pin for ") {
        "MessageDetails"
    } else {
        "PhotoViewer"
    };
    let parent = ElementHandle::find_by_element_type_name(ui, component)
        .next()
        .unwrap_or_else(|| panic!("missing {component} for {label}"));
    parent
        .query_descendants()
        .find_all()
        .into_iter()
        .find(|item| item.accessible_label().is_some_and(|value| value == label))
        .unwrap_or_else(|| panic!("missing viewer action {label}"))
        .invoke_accessible_default_action();
    slint::platform::update_timers_and_animations();
}

fn open(ui: &AppWindow, row: &MessageRow) {
    ui.invoke_close_message_actions();
    ui.set_action_error("".into());
    ui.set_photo_viewer_id(row.id);
    slint::platform::update_timers_and_animations();
    ui.set_photo_viewer_chat(ui.get_selected_id());
    ui.set_photo_viewer_message(row.clone());
    ui.set_photo_viewer_ready(true);
    ui.set_navigation_shown(ui.get_navigation_request());
}

fn finish(ui: &AppWindow, kind: u8, saved: bool) {
    presentation_actions::publish(
        ui,
        Some(&OperationReply {
            request: ui.get_action_request(),
            kind,
            saved,
            error: if saved {
                ""
            } else {
                "Synthetic backend failure"
            }
            .into(),
            ..Default::default()
        }),
        &Arc::from([]),
    );
    slint::platform::update_timers_and_animations();
}

// Called within the suite's single toolkit thread, using no account or device.
pub(crate) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    ui.set_live_can_send(true);
    ui.set_connection_state(5);
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    let _photos = crate::presentation_photos::bind(&ui);
    let commands = Rc::new(RefCell::new(Vec::new()));
    let accepting = Rc::new(Cell::new(true));
    presentation_actions::bind(&ui, {
        let commands = commands.clone();
        let accepting = accepting.clone();
        move |command| {
            if !accepting.get() {
                return false;
            }
            commands.borrow_mut().push(command);
            true
        }
    });
    let navigation = Rc::new(RefCell::new(Vec::new()));
    let navigation_accepting = Rc::new(Cell::new(false));
    crate::presentation_navigation::bind(&ui, {
        let navigation = navigation.clone();
        let accepting = navigation_accepting.clone();
        move |_, command| {
            if !accepting.get() {
                return false;
            }
            navigation.borrow_mut().push(command);
            true
        }
    });
    let downloads = Rc::new(RefCell::new(Vec::new()));
    ui.on_download_message({
        let downloads = downloads.clone();
        move |chat, message| downloads.borrow_mut().push((chat, message))
    });
    let copies = Rc::new(RefCell::new(Vec::new()));
    ui.on_copy_photo({
        let copies = copies.clone();
        move |message| copies.borrow_mut().push(message)
    });
    let mut demo = fixtures::demo();
    demo.select(9);
    let view = demo.conversation();
    presentation::conversation(&ui, &view, true);
    ui.show().unwrap();
    slint::platform::update_timers_and_animations();
    let mut row = presentation::message_row(&view.messages[0], None);
    row.id = PHOTO;
    row.wire_id = "synthetic-off-page-photo".into();
    row.actionable = true;
    row.has_photo = true;
    row.has_attachment = true;
    row.photo_state = 2;
    row.starred = false;
    row.pinned = false;
    assert!(!ui.get_timeline_rows().iter().any(|r| r.message.id == PHOTO));

    // Navigation closes only after its command is accepted.
    open(&ui, &row);
    let image_width = || {
        ElementHandle::find_by_element_id(&ui, "PhotoViewer::photo-image")
            .next()
            .unwrap()
            .size()
            .width
    };
    let fitted = image_width();
    activate(&ui, "Zoom in");
    assert!((image_width() - fitted * 2.).abs() < 1.);
    activate(&ui, "Zoom in");
    assert!((image_width() - fitted * 4.).abs() < 1.);
    activate(&ui, "Zoom in");
    assert!((image_width() - fitted * 4.).abs() < 1., "zoom is bounded");
    activate(&ui, "Zoom out");
    activate(&ui, "Zoom out");
    assert!((image_width() - fitted).abs() < 1.);
    activate(&ui, "Go to message");
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    assert!(ui.get_action_error().contains("busy"));
    assert!(navigation.borrow().is_empty());
    navigation_accepting.set(true);
    activate(&ui, "Go to message");
    assert_eq!(ui.get_photo_viewer_id(), 0);
    assert!(matches!(
        navigation.borrow().last(),
        Some(crate::message_navigation::Command::Navigate {
            chat: 9,
            message: PHOTO,
            quote: false,
            ..
        })
    ));

    // A rejected save or a stale acknowledgement leaves the viewer in place.
    open(&ui, &row);
    accepting.set(false);
    activate(&ui, "Reply to photo");
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    assert!(!ui.get_action_pending());
    assert!(ui.get_action_error().contains("busy"));
    accepting.set(true);
    activate(&ui, "Reply to photo");
    assert!(matches!(
        commands.borrow().last(),
        Some(Command::Reply {
            chat: 9,
            message: PHOTO,
            ..
        })
    ));
    let request = ui.get_action_request();
    presentation_actions::publish(
        &ui,
        Some(&OperationReply {
            request: request - 1,
            kind: 0,
            saved: true,
            ..Default::default()
        }),
        &Arc::from([]),
    );
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    assert!(ui.get_action_pending());
    finish(&ui, 0, false);
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    activate(&ui, "Reply to photo");
    finish(&ui, 0, true);
    assert_eq!(ui.get_photo_viewer_id(), 0);

    // Reaction and forwarding dialogs retain the selected photo behind them.
    open(&ui, &row);
    activate(&ui, "React to photo");
    assert_eq!(ui.get_message_dialog(), 1);
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    presentation::conversation(&ui, &view, false);
    assert_eq!(
        ui.get_message_dialog(),
        1,
        "archive refresh retains the off-page photo dialog"
    );
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    ui.invoke_react_message("👍".into());
    assert!(matches!(
        commands.borrow().last(),
        Some(Command::React { chat: 9, message: PHOTO, emoji, .. }) if emoji=="👍"
    ));
    finish(&ui, 1, true);
    assert_eq!(ui.get_message_dialog(), 0);
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    activate(&ui, "Forward photo");
    assert_eq!(ui.get_message_dialog(), 2);
    assert!(matches!(
        commands.borrow().last(),
        Some(Command::Destinations { .. })
    ));
    presentation_actions::publish(
        &ui,
        Some(&OperationReply {
            request: ui.get_action_request(),
            kind: 3,
            saved: true,
            ..Default::default()
        }),
        &Arc::from([Destination {
            id: 1,
            title: "Synthetic destination".into(),
        }]),
    );
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    ui.invoke_forward_message(1);
    assert!(matches!(
        commands.borrow().last(),
        Some(Command::Forward {
            chat: 9,
            message: PHOTO,
            destination: 1,
            ..
        })
    ));
    finish(&ui, 2, true);
    assert_eq!(ui.get_message_dialog(), 0);
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);

    // Pending star/pin work is backend-owned; no eager local toggle or duplicate.
    commands.borrow_mut().clear();
    activate(&ui, "Star photo");
    activate(&ui, "Star photo");
    assert_eq!(commands.borrow().len(), 1);
    assert!(matches!(
        commands.borrow().last(),
        Some(Command::Star {
            chat: 9,
            message: PHOTO,
            starred: true,
            ..
        })
    ));
    assert!(!ui.get_photo_viewer_message().starred);
    finish(&ui, 8, true);
    row.starred = true;
    ui.set_photo_viewer_message(row.clone());
    activate(&ui, "Unstar photo");
    assert!(matches!(
        commands.borrow().last(),
        Some(Command::Star { starred: false, .. })
    ));
    finish(&ui, 8, true);
    row.starred = false;
    ui.set_photo_viewer_message(row.clone());
    activate(&ui, "Pin photo");
    assert_eq!(ui.get_message_dialog(), 9);
    assert!(!ui.get_action_pending());
    activate(&ui, "Pin for 24 hours");
    assert_eq!(ui.get_pin_duration(), 86400);
    ui.invoke_confirm_message_control();
    assert!(matches!(
        commands.borrow().last(),
        Some(Command::Pin {
            chat: 9,
            message: PHOTO,
            seconds: 86400,
            ..
        })
    ));
    finish(&ui, 9, true);
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    row.pinned = true;
    ui.set_photo_viewer_message(row.clone());
    activate(&ui, "Unpin photo");
    assert!(matches!(
        commands.borrow().last(),
        Some(Command::Pin { seconds: 0, .. })
    ));
    finish(&ui, 9, true);

    assert!(
        !ui.get_action_pending(),
        "unpin completion releases the action gate"
    );
    assert_eq!(
        ui.get_message_dialog(),
        0,
        "unpin completion closes its dialog"
    );
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    assert_eq!(ui.get_photo_viewer_chat(), 9);
    assert_eq!(ui.get_selected_id(), 9);
    assert_eq!(ui.get_photo_viewer_message().id, PHOTO);
    assert!(ui.get_photo_viewer_message().actionable);
    assert_eq!(ui.get_download_phase(), 0, "no other download is active");
    activate(&ui, "Download photo");
    assert_eq!(
        *downloads.borrow(),
        [(9, PHOTO)],
        "download callback: action={}, chat={}, photo={}, pending={}, dialog={}, error={}",
        ui.get_action_message(),
        ui.get_action_chat(),
        ui.get_photo_viewer_id(),
        ui.get_action_pending(),
        ui.get_message_dialog(),
        ui.get_action_error()
    );
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    activate(&ui, "More photo options");
    for action in [0, 1, 2, 6, 7, 18, 19] {
        assert!(
            ui.get_message_menu_entries()
                .iter()
                .any(|entry| entry.action == action && entry.enabled)
        );
    }
    ui.invoke_choose_message_menu(18);
    assert_eq!(*copies.borrow(), [PHOTO]);
    assert_eq!(ui.get_message_dialog(), 0);
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);

    // The narrower toolbar keeps the same actions in More without losing context.
    ui.window().set_size(slint::LogicalSize::new(520., 600.));
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "Reply to photo")
            .next()
            .is_none()
    );
    activate(&ui, "More photo options");
    ui.invoke_choose_message_menu(1);
    assert_eq!(ui.get_photo_viewer_id(), PHOTO);
    assert!(matches!(
        commands.borrow().last(),
        Some(Command::Reply { message: PHOTO, .. })
    ));
    finish(&ui, 0, true);
    assert_eq!(ui.get_photo_viewer_id(), 0);

    // Old metadata and old results cannot act on or close a newly selected photo.
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    open(&ui, &row);
    activate(&ui, "Reply to photo");
    ui.set_photo_viewer_id(PHOTO + 1);
    let before = commands.borrow().len();
    ui.invoke_message_action(PHOTO, 7, 0., 0.);
    assert_eq!(commands.borrow().len(), before);
    finish(&ui, 0, true);
    assert_eq!(ui.get_photo_viewer_id(), PHOTO + 1);
    ui.set_photo_viewer_message(row.clone());
    ui.set_photo_viewer_chat(ui.get_selected_id());
    ui.invoke_message_action(PHOTO + 1, 7, 0., 0.);
    assert_eq!(
        commands.borrow().len(),
        before,
        "previous photo metadata is ineligible"
    );

    open(&ui, &row);
    ui.set_demo_mode(false);
    ui.set_live_shown_revision(ui.get_live_revision());
    ui.set_connection_state(0);
    row.starred = false;
    ui.set_photo_viewer_message(row);
    activate(&ui, "Star photo");
    assert!(!ui.get_action_pending());
    assert!(ui.get_action_error().contains("Connect"));
    assert_eq!(commands.borrow().len(), before);
    ui.hide().unwrap();
}
