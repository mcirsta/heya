use super::*;
use std::rc::Rc;
fn files(paths: &[&str]) -> slint::DataTransfer {
    let mut data = slint::DataTransfer::default();
    data.set_file_paths(paths.iter().map(std::path::Path::new));
    data
}
use slint::platform::{Key, WindowEvent};
fn key(ui: &AppWindow, value: impl Into<slint::SharedString>) {
    let text = value.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text });
}
fn shortcut(ui: &AppWindow, value: &str) {
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    key(ui, value);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
}
fn capture(ui: &AppWindow) -> Paste {
    Paste {
        target: target(ui),
        draft: ui.get_draft().into(),
        anchor: ui.get_composer_anchor(),
        cursor: ui.get_composer_cursor(),
        generation: ui.get_paste_generation(),
    }
}
pub fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    crate::presentation::conversation(&ui, &crate::fixtures::demo().conversation(), true);
    ui.window().set_size(slint::LogicalSize::new(1000., 700.));
    let (tx, mut rx) = mpsc::channel(8);
    bind(&ui, tx);
    let sends = Rc::new(std::cell::Cell::new(0));
    let sent = sends.clone();
    ui.on_send_message(move |_, _| sent.set(sent.get() + 1));
    ui.show().unwrap();
    ui.invoke_focus_composer();
    ui.set_draft("A🙂oldZ".into());
    ui.invoke_set_composer_cursor(5);
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    for _ in 0..3 {
        key(&ui, Key::RightArrow);
    }
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    shortcut(&ui, "v");
    let Command::Paste(paste) = rx.try_recv().unwrap() else {
        panic!("composer paste")
    };
    assert_eq!((paste.anchor, paste.cursor), (5, 8));
    assert!(
        rx.try_recv().is_err(),
        "one gesture must read the clipboard once"
    );
    ui.set_clipboard_busy(false);
    insert_text(&ui, paste, "new\nline");
    assert_eq!(ui.get_draft(), "A🙂new\nlineZ");
    assert_eq!(sends.get(), 0);
    shortcut(&ui, "z");
    assert_eq!(ui.get_draft(), "A🙂Z");
    // Slint's ordinary paste and composition commit both call TextInput::insert:
    // undoing a selection replacement removes the insertion, then restores the
    // deletion. Keep the toolkit's undo semantics rather than a separate stack.
    shortcut(&ui, "z");
    assert_eq!(
        ui.get_draft(),
        "A🙂oldZ",
        "native undo must retain the replaced selection"
    );
    let paste = capture(&ui);
    key(&ui, "!");
    let changed = ui.get_draft();
    insert_text(&ui, paste, "must not overwrite");
    assert_eq!(ui.get_draft(), changed);
    let paste = capture(&ui);
    ui.set_selected_id(2);
    insert_text(&ui, paste, "wrong recipient");
    assert_eq!(ui.get_draft(), changed);
    // A single-file and multi-file native drop share one captured recipient.
    ui.set_selected_id(1);
    ui.set_draft("Keep my text".into());
    ui.invoke_drop_files(files(&["/tmp/one.png", "/tmp/two 你好.txt"]));
    ui.set_selected_id(2);
    let Command::Drop(target, paths) = rx.try_recv().unwrap() else {
        panic!("drop")
    };
    assert_eq!(target.chat, 1);
    assert_eq!(paths.len(), 2);
    assert_eq!(ui.get_draft(), "Keep my text");
    assert_eq!(sends.get(), 0);
    ui.set_clipboard_busy(false);
    ui.set_attachment_phase(3);
    ui.invoke_drop_files(files(&["/tmp/ignored.txt"]));
    assert!(
        rx.try_recv().is_err(),
        "another selection cannot be overwritten"
    );
    ui.set_attachment_phase(0);
    ui.set_settings_open(true);
    ui.invoke_drop_files(files(&["/tmp/settings.txt"]));
    assert!(rx.try_recv().is_err());
    // Search fields keep ordinary text paste and never enter the upload path.
    ui.set_settings_open(false);
    let search =
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&ui, "Search chats")
            .next()
            .unwrap();
    let p = search.absolute_position();
    let size = search.size();
    let position = slint::LogicalPosition::new(p.x + size.width / 2., p.y + size.height / 2.);
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: slint::platform::PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: slint::platform::PointerEventButton::Left,
    });
    shortcut(&ui, "v");
    assert!(rx.try_recv().is_err());

    let mut demo = crate::fixtures::demo();
    demo.select(9);
    let view = demo.conversation();
    crate::presentation::conversation(&ui, &view, true);
    let id = view.messages[0].id;
    ui.invoke_copy_photo(id);
    assert!(
        rx.try_recv().is_err(),
        "unloaded previews are not originals"
    );
    crate::presentation_photos::publish(
        &ui,
        &std::sync::Arc::new(crate::photos::Snapshot {
            rows: [(
                id,
                crate::photos::Photo {
                    state: crate::photos::State::Ready,
                    image: Some(std::sync::Arc::new(crate::fixtures::photo_pixels(0, 64))),
                },
            )]
            .into(),
            viewer: None,
            ..Default::default()
        }),
    );
    ui.invoke_copy_photo(id);
    assert!(matches!(rx.try_recv().unwrap(),Command::Copy(9,copied) if copied==id));
    ui.hide().unwrap();
}

/// Real compositor -> winit -> Slint DropArea -> Heya callback. Kept separate
/// from the testing backend, with a synthetic sender and no account services.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires scripts/ui-offscreen.sh --wayland-drop-check"]
fn native_wayland_drop() {
    use std::{
        cell::Cell,
        time::{Duration, Instant},
    };
    assert_eq!(std::env::var("HEYA_PRIVATE_DROP_TEST").as_deref(), Ok("1"));
    assert!(std::env::var_os("DISPLAY").is_none(), "no X11 fallback");
    let mode = std::env::var("HEYA_TEST_DROP_MODE").unwrap();
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    crate::presentation::conversation(&ui, &crate::fixtures::demo().conversation(), true);
    ui.set_show_conversation(true);
    ui.set_draft("Preserve my draft".into());
    ui.window().set_size(slint::LogicalSize::new(900., 700.));
    let (tx, mut rx) = mpsc::channel(2);
    bind(&ui, tx);
    let sends = Rc::new(Cell::new(0));
    let sent = sends.clone();
    ui.on_send_message(move |_, _| sent.set(sent.get() + 1));
    let received = Rc::new(Cell::new(false));
    let result = received.clone();
    let hovered = Rc::new(Cell::new(false));
    let hover = hovered.clone();
    let weak = ui.as_weak();
    let start = Instant::now();
    let timer = slint::Timer::default();
    let expect_drop = mode == "drop";
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(20),
        move || {
            let ui = weak.upgrade().unwrap();
            hover.set(hover.get() || ui.get_drop_hover());
            if let Ok(command) = rx.try_recv() {
                assert!(expect_drop, "cancelled/invalid transfer must not import");
                let Command::Drop(target, paths) = command else {
                    panic!("expected drop")
                };
                assert_eq!(target.chat, ui.get_selected_id());
                assert_eq!(
                    paths,
                    [
                        PathBuf::from("/tmp/Sample 你好.png"),
                        PathBuf::from("/tmp/Notes.txt")
                    ]
                );
                assert_eq!(ui.get_draft(), "Preserve my draft");
                assert!(
                    rx.try_recv().is_err(),
                    "one transfer must produce one command"
                );
                result.set(true);
            }
            // Give the sender time to receive its completion, and the hover to clear.
            if start.elapsed() > Duration::from_secs(7) {
                assert_eq!(result.get(), expect_drop, "native drop did not arrive");
                assert!(!ui.get_drop_hover(), "hover survived drop/cancellation");
                slint::quit_event_loop().unwrap();
            }
        },
    );
    ui.run().unwrap();
    assert_eq!(received.get(), expect_drop);
    if mode != "text" {
        assert!(hovered.get(), "native drag never entered the conversation");
    }
    assert_eq!(sends.get(), 0, "a drop must never send automatically");
}
