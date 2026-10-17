//! Runs inside the shared offscreen Slint test thread.
use super::*;
use i_slint_backend_testing::ElementHandle;

fn layout(ui: &AppWindow) {
    for _ in 0..3 {
        ui.invoke_layout_timeline();
    }
}
fn items(ui: &AppWindow) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_type_name(ui, "TimelineItem")
        .filter(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
        .collect()
}

pub(super) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let mut demo = fixtures::demo();
    demo.select(12);
    let mut view = demo.conversation();
    for dark in [false, true] {
        ui.global::<crate::Theme>().set_dark(dark);
        for width in [520., 620., 800., 1200., 1920.] {
            ui.window().set_size(slint::LogicalSize::new(width, 1000.));
            conversation(&ui, &view, true);
            layout(&ui);
            assert_eq!(ui.get_timeline_rows().row_count(), 5);
            let mut count = 0;
            let mut rows = items(&ui);
            rows.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
            for item in &rows {
                if let Some(notice) = item
                    .query_descendants()
                    .match_type_name("SystemNotice")
                    .find_first()
                {
                    count += 1;
                    for kind in [
                        "MessageBubble",
                        "PhotoTile",
                        "AudioPlayer",
                        "TextInput",
                        "MessageHoverControls",
                        "MessageMenuArrow",
                        "TouchArea",
                    ] {
                        assert!(
                            notice
                                .query_descendants()
                                .match_type_name(kind)
                                .find_first()
                                .is_none(),
                            "notice allocated {kind}"
                        );
                    }
                    assert!(
                        item.query_descendants()
                            .match_type_name("MessageBubble")
                            .find_first()
                            .is_none()
                    );
                    let chip = notice
                        .query_descendants()
                        .match_id("SystemNotice::chip")
                        .find_first()
                        .unwrap();
                    let label = notice
                        .query_descendants()
                        .match_id("SystemNotice::label")
                        .find_first()
                        .unwrap();
                    let p = item.absolute_position();
                    let s = item.size();
                    let c = chip.absolute_position();
                    let cs = chip.size();
                    let l = label.absolute_position();
                    let ls = label.size();
                    assert!((c.x + cs.width / 2. - p.x - s.width / 2.).abs() < 0.3);
                    assert!(cs.width > 20. && c.x >= p.x && c.x + cs.width <= p.x + s.width + 0.3);
                    assert!(c.y >= p.y && c.y + cs.height <= p.y + s.height + 0.3);
                    assert!(l.y >= c.y && l.y + ls.height <= c.y + cs.height + 0.3);
                }
            }
            assert_eq!(count, 3);
            for pair in rows.windows(2) {
                assert!(
                    (pair[0].absolute_position().y + pair[0].size().height
                        - pair[1].absolute_position().y)
                        .abs()
                        < 0.3,
                    "width={width}: {:?}/{:?} then {:?}/{:?}; rows {:?}",
                    pair[0].absolute_position(),
                    pair[0].size(),
                    pair[1].absolute_position(),
                    pair[1].size(),
                    ui.get_timeline_rows()
                        .iter()
                        .map(|r| (r.message.id, r.top))
                        .collect::<Vec<_>>()
                );
            }
            let projected: Vec<_> = ui.get_timeline_rows().iter().collect();
            assert_eq!(projected[1].message.text, "Mira Sample pinned a message");
            assert_eq!(projected[2].message.text, "You pinned a message");
            assert_eq!(projected[3].message.text, "Mira Sample unpinned a message");
            assert!(
                projected[1..=3]
                    .iter()
                    .all(|r| r.message.system_notice && !r.message.actionable)
            );
            assert!(
                projected[4].message.first_in_run,
                "notice separates messages from the same sender"
            );
        }
    }
    // Bound shaping for pathological names, wrap Unicode, and remeasure after
    // content replacement without leaving a stale bubble or sender run.
    Arc::make_mut(&mut view.messages)[1].sender = "مرحبا 👩🏽‍💻 文字\n".repeat(100);
    ui.window().set_size(slint::LogicalSize::new(520., 1200.));
    conversation(&ui, &view, true);
    layout(&ui);
    let row = ui
        .get_timeline_rows()
        .iter()
        .find(|r| r.message.id == 25001)
        .unwrap();
    assert!(row.message.text.chars().count() < domain::SUMMARY_CHARS + 30);
    assert!(!row.message.text.contains('\n'));
    let changed = &mut Arc::make_mut(&mut view.messages)[1];
    changed.content = Default::default();
    changed.text = "Now an ordinary message".into();
    conversation(&ui, &view, false);
    layout(&ui);
    assert!(
        !ui.get_timeline_rows()
            .iter()
            .find(|r| r.message.id == 25001)
            .unwrap()
            .message
            .system_notice
    );

    // Thousands of notices keep the same bounded virtual window. Unchanged
    // snapshots cannot measure all history or create per-notice workers.
    demo.select(6);
    demo.chats[5].visible_count = 10_000;
    let mut long = demo.conversation();
    for m in Arc::make_mut(&mut long.messages) {
        m.content = Arc::new(crate::message_content::Content {
            notice: Some(crate::message_content::Notice::MessagePinned),
            ..Default::default()
        });
        m.text = "pinned a message".into();
    }
    for i in 0..12 {
        ui.window().set_size(slint::LogicalSize::new(
            if i % 2 == 0 { 620. } else { 1200. },
            800.,
        ));
        conversation(&ui, &long, i == 0);
        ui.set_timeline_y(-(i as f32 * 5000.));
        layout(&ui);
        assert!(ui.get_timeline_rows().row_count() < 100);
        assert!(items(&ui).len() < 100);
        assert!(
            ElementHandle::find_by_element_type_name(&ui, "MessageBubble")
                .all(|e| e.id().as_deref() == Some("AppWindow::measure-bubble"))
        );
        timeline::take_samples();
        conversation(&ui, &long, false);
        layout(&ui);
        assert_eq!(timeline::take_samples().measured_rows, 0);
    }
    long.messages = Arc::from([]);
    conversation(&ui, &long, false);
    layout(&ui);
    assert_eq!(ui.get_timeline_rows().row_count(), 0);
}
