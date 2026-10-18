//! Shared picker behavior against Slint's headless backend, using synthetic data.
use crate::{AppWindow, fixtures, presentation};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, rc::Rc};

pub fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    let demo = fixtures::demo();
    presentation::conversation(&ui, &demo.conversation(), true);
    ui.set_show_conversation(true);
    let sent = Rc::new(RefCell::new(Vec::new()));
    crate::presentation_actions::bind(&ui, {
        let sent = sent.clone();
        move |c| {
            sent.borrow_mut().push(c);
            true
        }
    });
    let edits = Rc::new(RefCell::new(Vec::new()));
    ui.on_draft_edited({
        let edits = edits.clone();
        move |v| edits.borrow_mut().push(v.to_string())
    });
    ui.show().unwrap();
    ui.invoke_layout_timeline();
    let baseline = ElementHandle::find_by_element_type_name(&ui, "ContentTarget").count();
    let geometry = (ui.get_timeline_y(), ui.get_timeline_height());
    ui.set_draft("你好".into());
    ui.invoke_set_composer_cursor(3);
    ElementHandle::find_by_accessible_label(&ui, "Emoji")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(ui.get_message_dialog(), 4);
    let panel = ElementHandle::find_by_element_id(&ui, "EmojiPicker::panel")
        .next()
        .unwrap();
    assert_eq!(panel.size(), slint::LogicalSize::new(440., 420.));
    let cells = ElementHandle::find_by_element_type_name(&ui, "ContentTarget").count() - baseline;
    assert!(
        cells < 160,
        "the grid must instantiate visible rows, not the complete emoji database: {cells}"
    );
    #[cfg(target_os = "macos")]
    {
        // A newer catalogue entry must not leave a blank picker cell on an
        // older OS. Newer fonts that support it should keep offering it.
        ui.set_emoji_query("distorted face".into());
        ui.invoke_request_emojis();
        let choices = ui
            .get_emoji_rows()
            .iter()
            .map(|row| row.cells.row_count())
            .sum::<usize>();
        assert_eq!(choices, usize::from(crate::macos::supports_emoji("🫪")));
        ui.set_emoji_query("".into());
        ui.invoke_request_emojis();
    }
    ui.invoke_choose_emoji("🙂".into());
    assert_eq!(ui.get_draft(), "你🙂好");
    ui.invoke_choose_emoji("👩🏽‍💻".into());
    assert_eq!(ui.get_draft(), "你🙂👩🏽‍💻好");
    assert_eq!(
        ui.get_message_dialog(),
        4,
        "composer picker stays open for repeated insertion"
    );
    // Incoming messages, receipts and sidebar updates republish conversations.
    // A composer picker has no message target and must survive that refresh.
    presentation::conversation(&ui, &demo.conversation(), false);
    assert_eq!(
        ui.get_message_dialog(),
        4,
        "background snapshots cannot close the composer picker"
    );
    assert_eq!(ui.get_draft(), "你🙂👩🏽‍💻好");
    assert!(
        sent.borrow().is_empty(),
        "inserting an emoji must never send a message/reaction"
    );
    assert_eq!(
        edits.borrow().len(),
        2,
        "each insertion follows normal draft autosave"
    );
    let original = ui.get_draft();
    ui.invoke_insert_composer_emoji("😀".into(), 1, 2);
    assert_eq!(
        ui.get_draft(),
        original,
        "invalid UTF-8 selections cannot corrupt the draft"
    );
    let end = original.len() as i32;
    ui.invoke_insert_composer_emoji("👍".into(), end, 0);
    assert_eq!(
        ui.get_draft(),
        "👍",
        "reversed selections replace the selected Unicode text"
    );
    let before = ui.get_draft();
    let selection = ui.get_emoji_selection();
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::DownArrow.into(),
        });
    assert_eq!(ui.get_emoji_selection(), selection + ui.get_emoji_columns());
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Return.into(),
        });
    assert_ne!(
        ui.get_draft(),
        before,
        "Enter in emoji search must insert the highlighted emoji"
    );
    assert!(sent.borrow().is_empty());
    ui.set_emoji_query("absolutely_no_matching_emoji_42".into());
    ui.invoke_request_emojis();
    assert_eq!(ui.get_emoji_rows().row_count(), 1);
    assert_eq!(
        ui.get_emoji_rows().row_data(0).unwrap().heading,
        "Nothing matches"
    );
    let draft = ui.get_draft();
    ui.invoke_navigate_emoji(0);
    assert_eq!(ui.get_draft(), draft);
    ui.invoke_jump_emoji_category(9);
    assert!(ui.get_emoji_query().is_empty());
    assert!(ui.get_emoji_scroll_row() > 0);
    let flag = ui
        .get_emoji_rows()
        .iter()
        .flat_map(|r| r.cells.iter().collect::<Vec<_>>())
        .find(|c| c.index == ui.get_emoji_selection())
        .unwrap();
    assert_eq!(
        emojis::get(&flag.emoji).unwrap().group(),
        emojis::Group::Flags
    );
    let cells = ElementHandle::find_by_element_type_name(&ui, "ContentTarget").count() - baseline;
    assert!(
        cells < 160,
        "category navigation must remain virtualized: {cells}"
    );
    for (width, height) in [(1200., 900.), (620., 700.), (520., 480.)] {
        ui.window().set_size(slint::LogicalSize::new(width, height));
        let p = ElementHandle::find_by_element_id(&ui, "EmojiPicker::panel")
            .next()
            .unwrap();
        let xy = p.absolute_position();
        let size = p.size();
        assert!(
            xy.x >= 7.5
                && xy.y >= 7.5
                && xy.x + size.width <= width - 7.5
                && xy.y + size.height <= height - 7.5,
            "picker must stay inside the window: window={width}x{height}, position={xy:?}, size={size:?}"
        );
    }
    ui.invoke_close_message_actions();
    assert_eq!(ui.get_message_dialog(), 0);
    assert!(
        ElementHandle::find_by_element_id(&ui, "EmojiPicker::panel")
            .next()
            .is_none()
    );
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    ui.invoke_open_composer_emoji(380., 830.);
    assert_eq!(ui.get_emoji_rows().row_data(0).unwrap().heading, "Recent");
    ui.invoke_close_message_actions();
    let id = ui.get_timeline_rows().row_data(0).unwrap().message.id;
    ui.invoke_message_action(id, 0, 400., 300.);
    assert_eq!(
        ui.get_emoji_rows().row_data(0).unwrap().heading,
        "Frequently Used"
    );
    ui.window().set_size(slint::LogicalSize::new(520., 480.));
    let narrow = ElementHandle::find_by_element_id(&ui, "EmojiPicker::panel")
        .next()
        .unwrap();
    let position = narrow.absolute_position();
    let size = narrow.size();
    assert!(
        position.x >= 8.
            && position.y >= 8.
            && position.x + size.width <= 512.
            && position.y + size.height <= 472.
    );
    assert!(
        ui.get_emoji_rows()
            .iter()
            .all(|r| r.cells.row_count() <= ui.get_emoji_columns() as usize)
    );
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    ui.invoke_search_message_actions("+1".into());
    assert!(
        ui.get_emoji_rows()
            .iter()
            .flat_map(|r| r.cells.iter().collect::<Vec<_>>())
            .any(|e| e.emoji == "👍")
    );
    ui.invoke_close_message_actions();
    assert_eq!(
        geometry,
        (ui.get_timeline_y(), ui.get_timeline_height()),
        "opening a picker cannot move the chat timeline"
    );
    eprintln!(
        "Synthetic emoji picker: {cells} materialized targets for {} emoji rows",
        ui.get_emoji_rows().row_count()
    );
    ui.hide().unwrap();
}
