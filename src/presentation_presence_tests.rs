use super::*;
use crate::{fixtures, presentation};
use i_slint_backend_testing::ElementHandle;

pub fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    presentation::conversation(&ui, &fixtures::demo().conversation(), true);
    ui.show().unwrap();
    for width in [1200., 900., 620., 520., 1920.] {
        ui.window().set_size(slint::LogicalSize::new(width, 800.));
        for title in [
            "Marcus",
            "A very long Unicode name · Ștefan 日本語 🦀 repeated repeatedly",
        ] {
            ui.set_chat_title(title.into());
            for status in ["online", "last seen yesterday at 23:58", ""] {
                ui.set_chat_presence(status.into());
                let find = |id| ElementHandle::find_by_element_id(&ui, id).next().unwrap();
                let area = find("AppWindow::header-info");
                let name = find("AppWindow::header-name");
                let area_end = area.absolute_position().x + area.size().width;
                assert!(name.size().width > 0.);
                assert!(name.absolute_position().x + name.size().width <= area_end + 0.5);
                assert!(
                    (name.absolute_position().y + name.size().height / 2.
                        - area.absolute_position().y
                        - area.size().height / 2.)
                        .abs()
                        < 0.5
                );
                if !status.is_empty() && area.size().width >= 140. {
                    let status = find("AppWindow::header-presence");
                    assert!(status.size().width > 0.);
                    assert!(
                        (status.absolute_position().x
                            - name.absolute_position().x
                            - name.size().width
                            - 12.)
                            .abs()
                            < 0.5
                    );
                    assert!(status.absolute_position().x + status.size().width <= area_end + 0.5);
                    assert!(
                        (status.absolute_position().y - name.absolute_position().y).abs() < 0.5
                    );
                }
            }
        }
    }
    // Actual callback gates, including a delayed update for the previous chat.
    ui.set_demo_mode(false);
    ui.set_live_chat_view(true);
    ui.set_live_revision(1);
    ui.set_live_shown_revision(1);
    ui.set_connection_state(5);
    ui.set_selected_id(1);
    let current = Snapshot {
        chat: 1,
        online: true,
        label: "online".into(),
    };
    let (tx, rx) = watch::channel(current.clone());
    let demand = bind(&ui, rx);
    ui.invoke_request_presence();
    assert_eq!(*demand.borrow(), 1);
    assert_eq!(ui.get_chat_presence(), "online");
    ui.set_settings_open(true);
    ui.invoke_request_presence();
    assert_eq!(*demand.borrow(), 0);
    assert!(ui.get_chat_presence().is_empty());
    ui.set_settings_open(false);
    ui.invoke_request_presence();
    assert_eq!(
        ui.get_chat_presence(),
        "online",
        "coalesced hide/show restores current slot"
    );
    ui.set_selected_id(2);
    ui.invoke_request_presence();
    publish(&ui, &current);
    assert_eq!(*demand.borrow(), 2);
    assert!(ui.get_chat_presence().is_empty());
    let next = Snapshot {
        chat: 2,
        online: false,
        label: "last seen today at 10:25".into(),
    };
    tx.send_replace(next.clone());
    publish(&ui, &next);
    assert_eq!(ui.get_chat_presence(), next.label);
    assert!(!ui.get_chat_online());
    let messages = ui.get_messages();
    let rows = ui.get_timeline_rows();
    let revision = ui.get_view_revision();
    let height = ui.get_timeline_height();
    let offset = ui.get_timeline_y();
    for i in 0..1000 {
        publish(
            &ui,
            &Snapshot {
                chat: 2,
                online: i % 2 == 0,
                label: if i % 2 == 0 {
                    "online"
                } else {
                    "last seen yesterday at 23:58"
                }
                .into(),
            },
        );
    }
    assert!(ui.get_messages() == messages);
    assert!(ui.get_timeline_rows() == rows);
    assert_eq!(ui.get_view_revision(), revision);
    assert_eq!(ui.get_timeline_height(), height);
    assert_eq!(ui.get_timeline_y(), offset);
    ui.set_connection_state(6);
    publish(&ui, &next);
    assert!(ui.get_chat_presence().is_empty());
    ui.set_demo_mode(true);
    conversation(&ui, 2, true);
    assert!(ui.get_chat_presence().is_empty());
    ui.hide().unwrap();
}
