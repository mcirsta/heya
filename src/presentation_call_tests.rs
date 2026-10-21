//! Shares the single Slint testing thread; no native window or account access.
use super::*;
use i_slint_backend_testing::ElementHandle;
pub(super) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let mut demo = fixtures::demo();
    demo.select(15);
    let view = demo.conversation();
    for dark in [false, true] {
        ui.global::<crate::Theme>().set_dark(dark);
        for width in [520., 800., 1200.] {
            ui.window().set_size(slint::LogicalSize::new(width, 1000.));
            conversation(&ui, &view, true);
            for _ in 0..3 {
                ui.invoke_layout_timeline();
            }
            assert_eq!(ui.get_timeline_rows().row_count(), 6);
            assert!(
                ui.get_timeline_rows()
                    .iter()
                    .all(|r| r.message.system_notice
                        && !r.message.actionable
                        && r.message.call_kind > 0)
            );
            let cards =
                ElementHandle::find_by_element_id(&ui, "CallEvent::card").collect::<Vec<_>>();
            assert_eq!(cards.len(), 6);
            for chip in cards {
                let p = chip.absolute_position();
                let s = chip.size();
                assert!(s.width >= 100. && s.width <= 300.5 && (s.height - 80.).abs() < 0.5);
                for label in chip.query_descendants().match_type_name("Text").find_all() {
                    let lp = label.absolute_position();
                    let ls = label.size();
                    assert!(lp.x >= p.x - 0.5 && lp.x + ls.width <= p.x + s.width + 0.5);
                    assert!(lp.y >= p.y - 0.5 && lp.y + ls.height <= p.y + s.height + 0.5);
                }
            }
        }
    }
    demo.select(6);
    demo.chats[5].visible_count = 10_000;
    fixtures::fill_calls(&mut demo.chats[5].messages);
    let view = demo.conversation();
    conversation(&ui, &view, true);
    for _ in 0..3 {
        ui.invoke_layout_timeline();
    }
    assert!(ui.get_timeline_rows().row_count() < 100);
    timeline::take_samples();
    conversation(&ui, &view, false);
    assert_eq!(timeline::take_samples().measured_rows, 0);
    assert!(
        ElementHandle::find_by_element_type_name(&ui, "MessageBubble")
            .all(|e| e.id().as_deref() == Some("AppWindow::measure-bubble")),
        "call rows must not instantiate ordinary message bubbles"
    );
}
