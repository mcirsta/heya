//! Toolkit checks use synthetic state and never a protocol client.
use crate::{AppWindow, fixtures, live_presentation, messaging::Command, presentation};
use slint::ComponentHandle;

pub fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let (tx, mut commands) = tokio::sync::mpsc::channel(32);
    live_presentation::bind_messages(&ui, tx);
    let mut demo = fixtures::demo();
    demo.select(6);
    presentation::conversation(&ui, &demo.conversation(), true);
    ui.set_live_chat_view(true);
    ui.set_live_revision(1);
    ui.set_live_shown_revision(1);
    ui.set_demo_mode(false);
    ui.set_read_window_active(true);
    ui.show().unwrap();
    ui.invoke_layout_timeline();
    let last = ui.get_read_tail();
    assert!(last > 0);
    assert_eq!(ui.get_read_visible_message(), last);
    for flag in 0..8 {
        match flag {
            0 => ui.set_read_window_active(false),
            1 => ui.set_read_window_occluded(true),
            2 => ui.set_settings_open(true),
            3 => ui.set_photo_viewer_id(1),
            4 => ui.set_message_dialog(1),
            5 => ui.set_live_shown_revision(0),
            6 => ui.set_demo_mode(true),
            _ => ui.set_new_chat_open(true),
        }
        assert_eq!(ui.get_read_visible_message(), 0);
        ui.invoke_read_visible();
        assert!(commands.try_recv().is_err());
        ui.set_read_window_active(true);
        ui.set_read_window_occluded(false);
        ui.set_settings_open(false);
        ui.set_photo_viewer_id(0);
        ui.set_message_dialog(0);
        ui.set_live_shown_revision(1);
        ui.set_demo_mode(false);
        ui.set_new_chat_open(false);
    }
    // Historical scroll position must not clear newly arrived messages below it.
    ui.set_timeline_y(0.);
    ui.invoke_layout_timeline();
    assert!(
        matches!(commands.try_recv().unwrap(),Command::Older {chat,revision:1,..}
        if chat==ui.get_selected_id()),
        "reaching history loads a page, not a read receipt"
    );
    assert_eq!(ui.get_read_tail(), 0);
    assert_eq!(ui.get_read_visible_message(), 0);
    ui.invoke_follow_timeline_bottom();
    ui.invoke_layout_timeline();
    assert_eq!(ui.get_read_visible_message(), last);
    // The single timer rechecks visibility when it fires, including a focus
    // loss after the dwell starts. No per-message timers or tasks are created.
    ui.invoke_read_visible();
    ui.set_read_window_active(false);
    std::thread::sleep(std::time::Duration::from_millis(220));
    slint::platform::update_timers_and_animations();
    assert!(commands.try_recv().is_err());
    ui.set_read_window_active(true);
    ui.invoke_read_visible();
    std::thread::sleep(std::time::Duration::from_millis(220));
    slint::platform::update_timers_and_animations();
    assert!(
        matches!(commands.try_recv().unwrap(),Command::ReadVisible {chat,message,revision:1} if chat==ui.get_selected_id() && message==last)
    );
    assert!(commands.try_recv().is_err());
    let mut view = demo.conversation();
    view.chat.unread = 0;
    let mut messages = view.messages.to_vec();
    messages[0].text.push_str(" (late update)");
    view.messages = messages.into();
    assert!(presentation::conversation(&ui, &view, false));
    ui.invoke_layout_timeline();
    assert_eq!(ui.get_read_tail(), last);
    std::thread::sleep(std::time::Duration::from_millis(220));
    slint::platform::update_timers_and_animations();
    assert!(
        matches!(commands.try_recv().unwrap(),Command::ReadVisible{message,..} if message==last)
    );
    // Deleted placeholders remain visible timeline rows and valid read tails.
    let tail = std::sync::Arc::make_mut(&mut view.messages)
        .last_mut()
        .unwrap();
    tail.text = "This message was deleted".into();
    std::sync::Arc::make_mut(&mut tail.content).unavailable = true;
    view.chat.unread = 1;
    assert!(presentation::conversation(&ui, &view, false));
    ui.invoke_layout_timeline();
    assert_eq!(ui.get_read_visible_message(), last);
    std::thread::sleep(std::time::Duration::from_millis(220));
    slint::platform::update_timers_and_animations();
    assert!(
        matches!(commands.try_recv().unwrap(), Command::ReadVisible{message,..} if message==last)
    );
    ui.hide().unwrap();
}
