use crate::{AppWindow, Theme, fixtures, message_content::Target, presentation};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model};
use std::{cell::Cell, rc::Rc, sync::Arc};

fn child(parent: &ElementHandle, id: &str) -> ElementHandle {
    parent
        .query_descendants()
        .match_id(id)
        .find_first()
        .unwrap_or_else(|| panic!("missing {id} below {:?}", parent.id()))
}

fn contains(parent: &ElementHandle, item: &ElementHandle) {
    let p = parent.absolute_position();
    let c = item.absolute_position();
    let ps = parent.size();
    let cs = item.size();
    assert!(
        c.x >= p.x - 0.5
            && c.y >= p.y - 0.5
            && c.x + cs.width <= p.x + ps.width + 0.5
            && c.y + cs.height <= p.y + ps.height + 0.5,
        "{:?} {c:?}/{cs:?} escapes {:?} {p:?}/{ps:?}",
        item.id(),
        parent.id()
    );
}

// Shares the one toolkit test thread; never launches a browser or reads a desktop clipboard.
pub(crate) fn check_ui() {
    check_same_name_participants();
    check_documents();
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let mut demo = fixtures::demo();
    demo.select(7);
    let mut view = demo.conversation();
    let clicked = Rc::new(Cell::new((0, 0)));
    ui.on_activate_message({
        let clicked = clicked.clone();
        move |id, index| clicked.set((id, index))
    });
    let demand = super::bind_images(&ui);
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for (width, height) in [
            (1200., 800.),
            (800., 600.),
            // Enough height to inspect all children; clipped elements are
            // intentionally absent from the toolkit's introspection API.
            (620., 700.),
            (520., 700.),
            (1920., 1080.),
        ] {
            ui.window().set_size(slint::LogicalSize::new(width, height));
            presentation::conversation(&ui, &view, true);
            ui.invoke_layout_timeline();
            let mut count = 0;
            for item in ElementHandle::find_by_element_type_name(&ui, "TimelineItem")
                .filter(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
            {
                let bubble = child(&item, "MessageBubble::bubble");
                contains(&item, &bubble);
                contains(&bubble, &child(&item, "MessageBubble::stamp"));
                if let Some(preview) = item
                    .query_descendants()
                    .match_type_name("PreviewCard")
                    .find_first()
                {
                    count += 1;
                    contains(&bubble, &preview);
                    assert!((preview.size().height - 88.).abs() < 0.5);
                    contains(&preview, &child(&preview, "PreviewCard::title-label"));
                    contains(&preview, &child(&preview, "PreviewCard::description-label"));
                    let target = preview
                        .query_descendants()
                        .match_type_name("ContentTarget")
                        .find_first()
                        .unwrap();
                    assert!((target.size().width - preview.size().width).abs() < 0.5);
                    assert!((target.size().height - preview.size().height).abs() < 0.5);
                    target.invoke_accessible_default_action();
                    assert_eq!(clicked.get(), (20001, -1));
                }
                let actions: Vec<_> = item
                    .query_descendants()
                    .match_type_name("MessageAction")
                    .find_all();
                if !actions.is_empty() {
                    let stamp = child(&item, "MessageBubble::stamp");
                    assert!(
                        stamp.absolute_position().y + stamp.size().height
                            <= actions[0].absolute_position().y
                    );
                }
                for (index, action) in actions.into_iter().enumerate() {
                    contains(&bubble, &action);
                    assert!(action.size().height >= 44.);
                    contains(&action, &child(&action, "MessageAction::label"));
                    assert!(
                        (child(&action, "MessageAction::divider")
                            .absolute_position()
                            .y
                            - action.absolute_position().y)
                            .abs()
                            < 0.5
                    );
                    let target = action
                        .query_descendants()
                        .match_type_name("ContentTarget")
                        .find_first()
                        .unwrap();
                    target.invoke_accessible_default_action();
                    assert_eq!(clicked.get(), (20002, index as i32));
                }
            }
            assert_eq!(count, 1);
            assert_eq!(*demand.borrow(), vec![20001]);
            let geometry = (ui.get_timeline_height(), ui.get_timeline_y());
            let images = fixtures::message_image_samples([20001]);
            super::publish_images(&ui, &images);
            let row = ui
                .get_timeline_rows()
                .iter()
                .find(|r| r.message.id == 20001)
                .unwrap();
            assert_eq!(row.message.thumbnail.size().width, 128);
            super::publish_images(&ui, &images);
            assert_eq!(
                row.message.thumbnail,
                ui.get_timeline_rows()
                    .iter()
                    .find(|r| r.message.id == 20001)
                    .unwrap()
                    .message
                    .thumbnail
            );
            super::publish_images(&ui, &Default::default());
            assert_eq!(
                geometry,
                (ui.get_timeline_height(), ui.get_timeline_y()),
                "thumbnail arrival/removal must not reflow or scroll"
            );
        }
    }
    assert_eq!(
        presentation::timeline::action(&ui, 20002, 0),
        Target::Url("https://example.invalid/delivery".into())
    );
    assert_eq!(
        presentation::timeline::action(&ui, 20002, 100),
        Target::Copy("https://example.invalid/delivery".into())
    );
    assert_eq!(
        presentation::timeline::action(&ui, 20002, 1),
        Target::Copy("DEMO-42".into())
    );
    assert_eq!(
        presentation::timeline::action(&ui, 20001, -3),
        Target::Copy("https://example.invalid/community/news".into())
    );
    assert_eq!(
        presentation::timeline::action(&ui, 20002, 99),
        Target::Unavailable
    );
    // Long Unicode labels, outgoing/group cards, missing metadata/images, repeated resizing.
    let messages = Arc::make_mut(&mut view.messages);
    messages[0].outgoing = true;
    let content = Arc::make_mut(&mut messages[0].content);
    let preview = content.preview.as_mut().unwrap();
    preview.title.clear();
    preview.description = "👩🏽‍💻 Salut · مرحبا · こんにちは ".repeat(100);
    preview.has_thumbnail = false;
    let content = Arc::make_mut(&mut messages[1].content);
    content.actions[0].label = "A longer multilingual label · こんにちは · مرحبا ".repeat(10);
    content.actions[1].target = Target::Unavailable;
    view.is_group = true;
    for width in [520., 800., 1200., 620.] {
        ui.window().set_size(slint::LogicalSize::new(width, 600.));
        presentation::conversation(&ui, &view, true);
        for item in ElementHandle::find_by_element_type_name(&ui, "MessageAction") {
            assert!(
                item.size().height < 90.,
                "long action label must be capped at two lines"
            );
            contains(&item, &child(&item, "MessageAction::label"));
        }
    }
    // Exercise Slint's copy path against the test backend's private clipboard,
    // then paste into the composer. This never changes the desktop clipboard.
    use slint::platform::{Key, PointerEventButton, WindowEvent};
    ui.invoke_copy_value("DEMO-42 👩🏽‍💻".into());
    let editor = ElementHandle::find_by_accessible_label(&ui, "Message draft")
        .next()
        .unwrap();
    let point = editor.absolute_position();
    let position = slint::LogicalPosition::new(
        point.x + editor.size().width / 2.,
        point.y + editor.size().height / 2.,
    );
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: "v".into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: "v".into() });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert_eq!(ui.get_draft(), "DEMO-42 👩🏽‍💻");
    view.messages = Arc::from([]);
    presentation::conversation(&ui, &view, true);
    assert!(demand.borrow().is_empty());
    assert_eq!(
        presentation::timeline::action(&ui, 20002, 0),
        Target::Unavailable,
        "removed message must not leave an executable action"
    );
}

fn check_documents() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let demand = super::bind_images(&ui);
    let mut demo = fixtures::demo();
    demo.select(11);
    let mut view = demo.conversation();
    let samples = view.messages.clone();
    let low = Arc::new(crate::photos::Pixels {
        width: 32,
        height: 13,
        rgba: vec![128; 32 * 13 * 4],
    });
    let page = fixtures::document_thumbnail();
    let strip = image::imageops::crop_imm(
        &page,
        0,
        0,
        crate::document_previews::WIDTH,
        crate::document_previews::HEIGHT,
    )
    .to_image();
    let image = Arc::new(crate::photos::Pixels {
        width: strip.width(),
        height: strip.height(),
        rgba: strip.into_raw(),
    });
    for (index, sample) in samples.iter().enumerate() {
        let mut message = sample.clone();
        // Bound display text without losing the full name in the domain/transcript.
        let full_name = "Project_👩🏽‍💻_こんにちは_مرحبا_".repeat(40) + ".pdf";
        Arc::make_mut(&mut message.content)
            .document
            .as_mut()
            .unwrap()
            .file_name = full_name.clone();
        assert!(message.transcript_line().contains(&full_name));
        for dark in [false, true] {
            ui.global::<Theme>().set_dark(dark);
            for (width, outgoing, group) in [
                (1200., false, false),
                (800., true, false),
                (620., false, true),
                (520., false, false),
                (360., false, true),
                (1200., true, true),
            ] {
                message.outgoing = outgoing;
                view.is_group = group;
                view.messages = vec![message.clone()].into();
                ui.window().set_size(slint::LogicalSize::new(width, 800.));
                presentation::conversation(&ui, &view, true);
                ui.invoke_layout_timeline();
                let item = ElementHandle::find_by_element_type_name(&ui, "TimelineItem")
                    .find(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
                    .unwrap();
                let bubble = child(&item, "MessageBubble::bubble");
                let card = child(&item, "MessageBubble::document");
                let info = child(&card, "DocumentCard::info");
                let name = child(&card, "DocumentCard::name");
                let detail = child(&card, "DocumentCard::detail");
                let stamp = child(&item, "MessageBubble::stamp");
                contains(&bubble, &card);
                contains(&card, &info);
                if index < 2 {
                    let preview = child(&card, "DocumentCard::preview");
                    contains(&card, &preview);
                    assert!(
                        (preview.absolute_position().y - card.absolute_position().y).abs() < 0.5
                    );
                    assert!(
                        (preview.absolute_position().y + preview.size().height
                            - info.absolute_position().y)
                            .abs()
                            < 0.5,
                        "preview must end before filename/details"
                    );
                }
                contains(&info, &name);
                contains(&info, &detail);
                contains(&bubble, &stamp);
                assert!(name.size().height <= 38.5);
                assert!(
                    detail.absolute_position().y >= name.absolute_position().y + name.size().height
                );
                assert!(
                    stamp.absolute_position().y >= card.absolute_position().y + card.size().height
                );
                assert!(card.size().height <= 224.5);
                if index == 2 {
                    assert!((card.size().height - 84.).abs() < 0.5);
                    let caption = child(&item, "MessageBubble::body");
                    assert!(
                        caption.absolute_position().y
                            >= card.absolute_position().y + card.size().height
                    );
                    assert!(
                        caption.absolute_position().y + caption.size().height
                            <= stamp.absolute_position().y
                    );
                }
                assert_eq!(demand.borrow().len(), usize::from(index < 2));
                let geometry = (
                    ui.get_timeline_y(),
                    ui.get_timeline_height(),
                    card.size().height,
                );
                let rows = ui.get_timeline_rows();
                let low_images = [(message.id, low.clone())]
                    .into_iter()
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into();
                super::publish_images(&ui, &low_images);
                let images = [(message.id, image.clone())]
                    .into_iter()
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into();
                super::publish_images(&ui, &images);
                assert_eq!(
                    rows,
                    ui.get_timeline_rows(),
                    "preview upgrades retain the virtualized model"
                );

                ui.invoke_layout_timeline();
                assert_eq!(
                    geometry,
                    (
                        ui.get_timeline_y(),
                        ui.get_timeline_height(),
                        card.size().height
                    )
                );
                super::publish_images(&ui, &Default::default());
                assert_eq!(
                    geometry,
                    (
                        ui.get_timeline_y(),
                        ui.get_timeline_height(),
                        card.size().height
                    )
                );
            }
        }
    }
    let mut many = demo.chats[5].messages.clone();
    fixtures::fill_documents(&mut many);
    view.messages = many.into();
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    presentation::conversation(&ui, &view, true);
    for y in [0., -2000., -10000., -30000.] {
        ui.set_timeline_y(y);
        ui.invoke_layout_timeline();
        assert!(
            ui.get_timeline_rows().row_count() < 24,
            "10,000 documents must stay virtualized"
        );
        assert!(demand.borrow().len() <= crate::message_thumbnails::MAX_DEMAND);
    }
    view.messages = Arc::from([]);
    presentation::conversation(&ui, &view, true);
    assert!(demand.borrow().is_empty());
}

// A new author with an identical name must invalidate the cached run geometry.
fn check_same_name_participants() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let mut demo = fixtures::demo();
    demo.select(2);
    let mut view = demo.conversation();
    view.messages = view.messages[..2].to_vec().into();
    for message in Arc::make_mut(&mut view.messages) {
        message.sender = "Alex 👩🏽‍💻".into();
        message.outgoing = false;
    }
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for width in [520., 800., 1200.] {
            ui.window().set_size(slint::LogicalSize::new(width, 900.));
            presentation::conversation(&ui, &view, true);
            ui.invoke_layout_timeline();
            let rows: Vec<_> = ui.get_timeline_rows().iter().collect();
            assert_eq!(rows.len(), 2);
            assert!(rows.iter().all(|r| r.message.first_in_run));
            for item in ElementHandle::find_by_element_type_name(&ui, "TimelineItem")
                .filter(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
            {
                let bubble = child(&item, "MessageBubble::bubble");
                contains(&bubble, &child(&item, "MessageBubble::sender"));
                contains(&bubble, &child(&item, "MessageBubble::stamp"));
            }
            // Keep the current rows and update only identity/name. The second
            // row now belongs to the same person, despite their changed name.
            let mut same_person = view.clone();
            let messages = Arc::make_mut(&mut same_person.messages);
            messages[1].sender_id = messages[0].sender_id.clone();
            messages[1].sender = "Alex renamed".into();
            presentation::conversation(&ui, &same_person, true);
            ui.invoke_layout_timeline();
            assert!(
                !ui.get_timeline_rows()
                    .row_data(1)
                    .unwrap()
                    .message
                    .first_in_run
            );
        }
    }
}
