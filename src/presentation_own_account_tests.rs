use super::*;
use crate::own_account::Reply;
use i_slint_backend_testing::ElementHandle;
use std::rc::Rc;

pub fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    ui.window().set_size(slint::LogicalSize::new(1000., 700.));
    let (commands, mut requests) = mpsc::channel(4);
    bind(&ui, commands);
    let prefs = Rc::new(std::cell::Cell::new(-1));
    let changed = prefs.clone();
    let weak = ui.as_weak();
    ui.on_preference_changed(move |key, value| {
        assert_eq!(key, "appear_offline");
        changed.set(value);
        weak.upgrade().unwrap().set_appear_offline(value == 1);
    });
    ui.invoke_choose_own_status(1);
    let command = requests.try_recv().unwrap();
    assert!(matches!(
        command.action,
        Action::Presence(Visibility::Invisible)
    ));
    assert!(ui.get_own_pending());
    ui.invoke_choose_own_status(0);
    assert!(
        requests.try_recv().is_err(),
        "double click must not enqueue a second action"
    );
    publish(
        &ui,
        &Snapshot {
            connected: true,
            visibility: Some(Visibility::Online),
            busy: true,
            ..Default::default()
        },
    );
    assert_eq!(
        ui.get_own_visibility(),
        0,
        "pending visibility must not claim success"
    );
    let mut state = Snapshot {
        connected: true,
        visibility: Some(Visibility::Invisible),
        reply: Some(Reply {
            request: command.request,
            kind: 0,
            error: String::new(),
        }),
        ..Default::default()
    };
    publish(&ui, &state);
    assert!(!ui.get_own_pending());
    assert_eq!(ui.get_own_visibility(), 1);
    assert_eq!(prefs.get(), 1);
    ui.invoke_edit_status_message();
    let load = requests.try_recv().unwrap();
    assert!(matches!(load.action, Action::LoadAbout));
    ui.set_own_about_draft("New draft while loading".into());
    ui.set_own_about_dirty(true);
    state.about = Some("Older profile value".into());
    state.reply = Some(Reply {
        request: load.request,
        kind: 1,
        error: String::new(),
    });
    publish(&ui, &state);
    assert_eq!(ui.get_own_about_draft(), "New draft while loading");
    ui.invoke_save_status_message();
    let save = requests.try_recv().unwrap();
    state.reply = Some(Reply {
        request: save.request,
        kind: 2,
        error: "Synthetic save failure".into(),
    });
    publish(&ui, &state);
    assert!(ui.get_own_editor_open());
    assert_eq!(ui.get_own_about_draft(), "New draft while loading");
    ui.invoke_save_status_message();
    let save = requests.try_recv().unwrap();
    ui.set_own_about_draft("Even newer unsaved text".into());
    state.about = Some("New draft while loading".into());
    state.reply = Some(Reply {
        request: save.request,
        kind: 2,
        error: String::new(),
    });
    publish(&ui, &state);
    assert!(
        ui.get_own_editor_open(),
        "a late save must preserve newer input"
    );
    ui.invoke_save_status_message();
    let save = requests.try_recv().unwrap();
    state.about = Some("Even newer unsaved text".into());
    state.reply = Some(Reply {
        request: save.request,
        kind: 2,
        error: String::new(),
    });
    publish(&ui, &state);
    assert!(!ui.get_own_editor_open());
    ui.show().unwrap();
    let header = || {
        ElementHandle::find_by_element_id(&ui, "AppWindow::conversation-content")
            .next()
            .unwrap()
            .absolute_position()
            .y
    };
    let original = header();
    ui.invoke_copy_value("Synthetic copy".into());
    assert!(ui.get_error_message().is_empty(), "copying must be silent");
    ui.invoke_show_error("Synthetic error".into());
    assert_eq!(
        header(),
        original,
        "errors must not reflow the conversation"
    );
    assert!(
        ElementHandle::find_by_element_id(&ui, "AppWindow::notice-bar")
            .next()
            .is_none()
    );
    ui.invoke_close_error();
    ui.hide().unwrap();
}
