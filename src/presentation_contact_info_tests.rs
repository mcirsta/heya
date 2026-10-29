//! Synthetic contact-panel input, lifecycle and layout checks on the toolkit thread.
use crate::{
    AppWindow, Theme,
    contact_info::{Demand, Info, Kind, Media, MediaList, Section, Snapshot},
    fixtures, photos, presentation, presentation_contact_info,
};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, rc::Rc, sync::Arc};

fn element(ui: &AppWindow, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, id)
        .next()
        .unwrap_or_else(|| {
            panic!(
                "missing {id}: open={}, target={}, selected={}, previews={}, browser={}, size={:?}",
                ui.get_contact_info_open(),
                ui.get_contact_info_chat(),
                ui.get_selected_id(),
                ui.get_contact_info_media().row_count(),
                ui.get_contact_info_media_view(),
                ui.window().size(),
            )
        })
}

fn contained(parent: &ElementHandle, child: &ElementHandle) {
    let p = parent.absolute_position();
    let c = child.absolute_position();
    let ps = parent.size();
    let cs = child.size();
    assert!(
        c.x >= p.x - 0.5
            && c.y >= p.y - 0.5
            && c.x + cs.width <= p.x + ps.width + 0.5
            && c.y + cs.height <= p.y + ps.height + 0.5,
        "{:?} {c:?}/{cs:?} escapes {:?} {p:?}/{ps:?}",
        child.id(),
        parent.id()
    );
}

fn settle(ui: &AppWindow) {
    ui.invoke_layout_timeline();
    slint::platform::update_timers_and_animations();
}

fn snapshot(demand: Demand, info: Info) -> Arc<Snapshot> {
    Arc::new(Snapshot {
        demand,
        info: Some(info),
        list: None,
        error: String::new(),
    })
}

pub(crate) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(false);
    ui.set_live_chat_view(true);
    ui.set_connection_state(5);
    ui.set_show_conversation(true);
    let mut demo = fixtures::demo();
    demo.select(9);
    presentation::conversation(&ui, &demo.conversation(), true);
    // The live publisher acknowledges this revision after applying the view.
    ui.set_live_shown_revision(ui.get_live_revision());
    let draft = "Unsent draft 👩🏽‍💻\nKeep all of this text.\nこんにちは";
    ui.set_draft(draft.into());
    let demand = presentation_contact_info::bind(&ui);
    let activated = Rc::new(RefCell::new(Vec::new()));
    ui.on_activate_photo({
        let activated = activated.clone();
        move |id| activated.borrow_mut().push(id)
    });
    let calls = Rc::new(RefCell::new(Vec::new()));
    ui.global::<crate::ui::AudioCall>().on_action({
        let calls = calls.clone();
        move |action| calls.borrow_mut().push(action)
    });
    let (download_commands, mut downloads) = tokio::sync::mpsc::channel(8);
    crate::presentation_downloads::bind(&ui, download_commands);
    let (video_commands, mut videos) = tokio::sync::mpsc::channel(8);
    crate::presentation_video::bind(&ui, video_commands);
    let links = Rc::new(RefCell::new(Vec::new()));
    ui.on_activate_message({
        let links = links.clone();
        move |id, action| links.borrow_mut().push((id, action))
    });
    let management = Rc::new(RefCell::new(Vec::new()));
    ui.on_manage_chat({
        let management = management.clone();
        move |chat, action| management.borrow_mut().push((chat, action))
    });
    let searches = Rc::new(RefCell::new(Vec::new()));
    ui.on_search_messages({
        let searches = searches.clone();
        move |query| searches.borrow_mut().push(query)
    });
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    ui.show().unwrap();
    element(&ui, "AppWindow::header-info").invoke_accessible_default_action();
    let request = demand.borrow().clone();
    assert!(ui.get_contact_info_open());
    assert!(ui.get_contact_info_loading());
    assert_eq!(request.chat, 9);
    assert_eq!(request.request, ui.get_contact_info_request());
    assert!(!request.media);
    let name = "A long synthetic contact 👩🏽‍💻 ăâîșț 日本語 ".repeat(8);
    let info = Info {
        chat: 9,
        name: name.clone(),
        phone: Some("+1 555 000 0001".into()),
        media_count: 8,
        document_count: 2,
        link_count: 1,
        shared_count: 11,
        photos: (90001..90009).collect(),
        ..Default::default()
    };
    let current = snapshot(request.clone(), info.clone());
    presentation_contact_info::publish(&ui, &current);
    assert!(!ui.get_contact_info_loading());
    assert_eq!(ui.get_contact_info_name(), name);
    assert_eq!(ui.get_contact_info_phone(), "+1 555 000 0001");
    assert_eq!(
        ui.get_contact_info_media().row_count(),
        crate::contact_info::PREVIEW_COUNT
    );
    assert_eq!(ui.get_draft(), draft);

    // Copy uses the toolkit's private test clipboard. The display field must
    // allow selection while rejecting edits, without changing contact data.
    check_phone_copy(&ui);
    ui.set_draft(draft.into());

    // Late/coalesced work cannot replace the panel for a new request or chat.
    for stale in [
        Demand {
            request: request.request + 1,
            ..request.clone()
        },
        Demand {
            chat: 10,
            ..request.clone()
        },
    ] {
        presentation_contact_info::publish(
            &ui,
            &snapshot(
                stale,
                Info {
                    name: "Late result".into(),
                    ..info.clone()
                },
            ),
        );
        assert_eq!(ui.get_contact_info_name(), name);
    }
    presentation_contact_info::publish(
        &ui,
        &Arc::new(Snapshot {
            demand: request.clone(),
            info: None,
            list: None,
            error: "Synthetic archive unavailable".into(),
        }),
    );
    assert!(!ui.get_contact_info_loading());
    assert!(ui.get_contact_info_open());
    assert_eq!(ui.get_contact_info_media().row_count(), 0);
    assert_eq!(ui.get_contact_info_items().row_count(), 0);
    assert_eq!(ui.get_contact_info_error(), "Synthetic archive unavailable");
    assert_eq!(ui.get_draft(), draft);
    presentation_contact_info::publish(&ui, &current);
    assert!(ui.get_contact_info_error().is_empty());
    check_layout(&ui);
    // A deferred chat-selection callback may stop the previous video. It must
    // never start one merely because the details panel was opened.
    while let Ok(command) = videos.try_recv() {
        assert!(matches!(command, crate::video::Command::Stop));
    }

    // Preview pixels come from the existing photo publisher. Unrequested IDs
    // must not grow the panel model or create additional toolkit images.
    let pixels = Arc::new(fixtures::photo_pixels(0, photos::INLINE_EDGE));
    let photo_data = Arc::new(photos::Snapshot {
        rows: (90001..90001 + photos::MAX_DEMAND as i32)
            .map(|id| {
                (
                    id,
                    photos::Photo {
                        state: photos::State::Ready,
                        image: Some(pixels.clone()),
                    },
                )
            })
            .collect(),
        ..Default::default()
    });
    crate::presentation_photos::publish(&ui, &photo_data);
    assert_eq!(ui.get_contact_info_media().row_count(), 4);
    assert!(
        ui.get_contact_info_media()
            .iter()
            .all(|row| row.thumbnail.size().width > 0)
    );
    let first_image = ui.get_contact_info_media().row_data(0).unwrap().thumbnail;
    crate::presentation_photos::publish(&ui, &photo_data);
    assert_eq!(
        first_image,
        ui.get_contact_info_media().row_data(0).unwrap().thumbnail,
        "unchanged photos reuse the same texture"
    );
    element(&ui, "ContactInfo::preview-strip")
        .query_descendants()
        .match_id("MediaTile::tile")
        .find_first()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*activated.borrow(), [90001]);
    ui.invoke_open_contact_media(90100, Kind::Photo as i32);
    ui.invoke_open_contact_media(90001, Kind::Video as i32);
    assert_eq!(*activated.borrow(), [90001]);
    assert!(
        calls.borrow().is_empty(),
        "opening contact info never calls"
    );

    for (tab, section) in [
        (0, Section::Media),
        (1, Section::Links),
        (2, Section::Documents),
    ] {
        ui.invoke_request_contact_media(tab);
        let next = demand.borrow().clone();
        assert!(next.media);
        assert_eq!(next.section, Some(section));
        assert_eq!(next.request, ui.get_contact_info_request());
        assert!(ui.get_contact_info_media_view());
        let row = Media {
            id: 90020,
            kind: match section {
                Section::Media => Kind::Video,
                Section::Documents => Kind::Document,
                Section::Links => Kind::Link,
            },
            label: "Synthetic item 👩🏽‍💻 日本語".into(),
            detail: "Saved history".into(),
            link: (section == Section::Links).then(|| "https://example.invalid/synthetic".into()),
        };
        presentation_contact_info::publish(
            &ui,
            &Arc::new(Snapshot {
                demand: next,
                info: Some(info.clone()),
                list: Some(MediaList {
                    chat: 9,
                    section: Some(section),
                    rows: vec![row],
                    more: false,
                }),
                error: String::new(),
            }),
        );
        assert_eq!(ui.get_contact_info_items().row_count(), 1);
        let kind = match section {
            Section::Media => Kind::Video,
            Section::Documents => Kind::Document,
            Section::Links => Kind::Link,
        };
        ui.invoke_open_contact_media(90020, kind as i32);
        match section {
            Section::Media => {
                assert!(matches!(
                    videos.try_recv().unwrap(),
                    crate::video::Command::Toggle(90020)
                ));
                ui.invoke_video_control(90100, 0, 0.);
                assert!(videos.try_recv().is_err());
                ui.set_video_expanded(false);
            }
            Section::Documents => {
                assert!(matches!(
                    downloads.try_recv().unwrap(),
                    crate::downloads::Command::Start(crate::downloads::Target {
                        chat: 9,
                        message: 90020,
                        ..
                    })
                ));
                ui.invoke_download_message(10, 90020);
                ui.invoke_download_message(9, 90100);
                assert!(downloads.try_recv().is_err());
            }
            Section::Links => {
                assert_eq!(*links.borrow(), [(90020, -1)]);
                assert_eq!(
                    ui.get_contact_info_items().row_data(0).unwrap().url,
                    "https://example.invalid/synthetic"
                );
            }
        }
        assert_eq!(ui.get_draft(), draft);
    }

    // All settings actions reuse the selected chat and existing action IDs;
    // these callbacks are captured, with no account or mutation worker running.
    ui.invoke_request_contact_media(-1);
    let summary_request = demand.borrow().clone();
    presentation_contact_info::publish(&ui, &snapshot(summary_request, info.clone()));
    element(&ui, "ContactInfo::scroll").scroll(0., -10000.);
    settle(&ui);
    element(&ui, "ContactInfo::mute-row").invoke_accessible_default_action();
    element(&ui, "ContactInfo::scroll").scroll(0., -10000.);
    settle(&ui);
    ElementHandle::find_by_accessible_label(&ui, "Always")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    ui.set_contact_info_muted(true);
    element(&ui, "ContactInfo::mute-row").invoke_accessible_default_action();
    element(&ui, "ContactInfo::archive-row").invoke_accessible_default_action();
    ui.set_contact_info_archived(true);
    element(&ui, "ContactInfo::archive-row").invoke_accessible_default_action();
    element(&ui, "ContactInfo::lock-row").invoke_accessible_default_action();
    assert_eq!(
        *management.borrow(),
        [(9, 2), (9, 3), (9, 4), (9, 5), (9, 6)]
    );
    assert_eq!(ui.get_draft(), draft);
    for label in ["Voice call", "Video call", "Search messages"] {
        element(&ui, "ContactInfo::scroll").scroll(0., 10000.);
        settle(&ui);
        ElementHandle::find_by_accessible_label(&ui, label)
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert!(!ui.get_contact_info_open());
        assert_eq!(demand.borrow().chat, 0);
        assert_eq!(ui.get_draft(), draft);
        if label == "Search messages" {
            assert!(ui.get_message_search_open());
            assert_eq!(searches.borrow().len(), 1);
            assert!(searches.borrow()[0].is_empty());
            ui.invoke_close_message_search();
        }
        ui.invoke_show_contact_info();
        let reopened = demand.borrow().clone();
        presentation_contact_info::publish(&ui, &snapshot(reopened, info.clone()));
        settle(&ui);
    }
    assert_eq!(*calls.borrow(), [0, 7]);
    element(&ui, "ContactInfo::close-button").invoke_accessible_default_action();
    assert!(!ui.get_contact_info_open());
    assert_eq!(demand.borrow().chat, 0);
    assert_eq!(ui.get_contact_info_media().row_count(), 0);
    assert_eq!(ui.get_contact_info_items().row_count(), 0);
    presentation_contact_info::publish(&ui, &current);
    crate::presentation_photos::publish(&ui, &photo_data);
    assert!(!ui.get_contact_info_open());
    assert_eq!(ui.get_contact_info_media().row_count(), 0);
    assert_eq!(ui.get_contact_info_items().row_count(), 0);
    assert_eq!(ui.get_draft(), draft);
    element(&ui, "AppWindow::header-avatar").invoke_accessible_default_action();
    assert!(ui.get_contact_info_open());
    assert_eq!(demand.borrow().chat, ui.get_selected_id());
    ui.invoke_close_contact_info();
    ui.hide().unwrap();
}

fn check_phone_copy(ui: &AppWindow) {
    use slint::platform::{Key, PointerEventButton, WindowEvent};
    fn key(ui: &AppWindow, text: impl Into<slint::SharedString>) {
        let text = text.into();
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    }
    fn shortcut(ui: &AppWindow, text: &str) {
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        key(ui, text);
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
    }
    let expected = ui.get_contact_info_phone();
    ui.invoke_copy_value("Private clipboard sentinel".into());
    let phone = element(ui, "ContactInfo::phone");
    let p = phone.absolute_position();
    let position = slint::LogicalPosition::new(
        p.x + phone.size().width / 2.,
        p.y + phone.size().height / 2.,
    );
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    shortcut(ui, "a");
    key(ui, "9");
    shortcut(ui, "c");
    ui.set_draft("".into());
    ui.invoke_focus_composer();
    shortcut(ui, "v");
    assert_eq!(
        ui.get_draft(),
        expected,
        "read-only phone selection copies the full number"
    );
    assert_eq!(ui.get_contact_info_phone(), expected);
}

fn check_layout(ui: &AppWindow) {
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for (width, height) in [(1200., 900.), (800., 720.), (520., 480.)] {
            ui.window().set_size(slint::LogicalSize::new(width, height));
            settle(ui);
            assert!(
                ui.get_contact_info_open(),
                "layout/selection callbacks must not close the current contact panel at {width}x{height}"
            );
            assert_eq!(
                ui.get_contact_info_media().row_count(),
                crate::contact_info::PREVIEW_COUNT,
                "layout must not discard current preview metadata at {width}x{height}"
            );
            element(ui, "ContactInfo::scroll").scroll(0., 10000.);
            settle(ui);
            let panel = ElementHandle::find_by_element_type_name(ui, "ContactInfo")
                .next()
                .unwrap();
            let p = panel.absolute_position();
            let size = panel.size();
            assert!(p.x >= 0. && p.y >= 0.);
            assert!(p.x + size.width <= width + 0.5);
            assert!(p.y + size.height <= height + 0.5);
            let header = element(ui, "ContactInfo::header");
            contained(&panel, &header);
            assert!(
                (header.absolute_position().y - p.y).abs() < 0.5,
                "contact header stays at the top"
            );
            contained(&header, &element(ui, "ContactInfo::close-button"));
            let profile = element(ui, "ContactInfo::profile");
            for id in [
                "ContactInfo::avatar",
                "ContactInfo::name",
                "ContactInfo::phone",
                "ContactInfo::voice-action",
                "ContactInfo::video-action",
                "ContactInfo::search-action",
            ] {
                contained(&profile, &element(ui, id));
            }
            // The small-height case puts previews below the fold. Exercise
            // the actual scroll area rather than querying clipped-away items.
            element(ui, "ContactInfo::scroll").scroll(0., -260.);
            settle(ui);
            let strip = element(ui, "ContactInfo::preview-strip");
            contained(&panel, &strip);
            let tiles = strip
                .query_descendants()
                .match_id("MediaTile::tile")
                .find_all();
            assert_eq!(tiles.len(), crate::contact_info::PREVIEW_COUNT);
            for tile in tiles {
                contained(&strip, &tile);
            }
        }
    }
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    settle(ui);
    element(ui, "ContactInfo::scroll").scroll(0., 10000.);
    settle(ui);
}
