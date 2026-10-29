use super::*;
use crate::{Theme, fixtures, presentation};
use i_slint_backend_testing::ElementHandle;

fn child(parent: &ElementHandle, id: &str) -> ElementHandle {
    parent
        .query_descendants()
        .match_id(id)
        .find_first()
        .unwrap_or_else(|| panic!("missing {id}"))
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
// Runs within the suite's one toolkit thread; synthetic pixels, private test backend.
pub(crate) fn check_ui() {
    check_grid();
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let demand = bind(&ui);
    let mut demo = fixtures::demo();
    demo.select(9);
    let mut view = demo.conversation();
    let messages = view.messages.clone();
    let sample = Arc::new(fixtures::photo_pixels(0, photos::INLINE_EDGE));
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for (width, height) in [(1200., 1000.), (520., 1000.), (800., 1100.), (1920., 1080.)] {
            ui.window().set_size(slint::LogicalSize::new(width, height));
            for message in messages.iter() {
                view.messages = Arc::from([message.clone()]);
                presentation::conversation(&ui, &view, true);
                ui.invoke_layout_timeline();
                assert_eq!(demand.borrow().visible, vec![message.id]);
                let before = (ui.get_timeline_height(), ui.get_timeline_y());
                for state in [
                    photos::State::Idle,
                    photos::State::Loading,
                    photos::State::Ready,
                    photos::State::Failed,
                ] {
                    let data = Arc::new(photos::Snapshot {
                        rows: [(
                            message.id,
                            photos::Photo {
                                state,
                                image: Some(sample.clone()),
                            },
                        )]
                        .into(),
                        viewer: None,
                        ..Default::default()
                    });
                    publish(&ui, &data);
                    assert_eq!(
                        before,
                        (ui.get_timeline_height(), ui.get_timeline_y()),
                        "image/state changes must not reflow the chat"
                    );
                    let item = ElementHandle::find_by_element_type_name(&ui, "TimelineItem")
                        .find(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
                        .unwrap();
                    let bubble = child(&item, "MessageBubble::bubble");
                    let photo = child(&item, "MessageBubble::photo");
                    let stamp = child(&item, "MessageBubble::stamp");
                    contains(&item, &bubble);
                    contains(&bubble, &photo);
                    contains(&bubble, &stamp);
                    assert!(photo.size().height <= 440.5 && photo.size().width <= 320.5);
                    if state == photos::State::Failed {
                        let disc = child(&photo, "PhotoTile::status-disc");
                        contains(&photo, &disc);
                        assert_eq!(disc.size(), slint::LogicalSize::new(44., 44.));
                        assert!(
                            disc.query_descendants()
                                .match_type_name("Text")
                                .find_all()
                                .is_empty(),
                            "no loading banner over the photo"
                        );
                    } else {
                        assert!(
                            photo
                                .query_descendants()
                                .match_id("PhotoTile::status-disc")
                                .find_all()
                                .is_empty(),
                            "automatic photo loading has no overlay"
                        );
                    }
                    if message.content.body.as_deref() == Some("") {
                        contains(&photo, &stamp);
                    } else {
                        let body = child(&item, "MessageBubble::body");
                        contains(&bubble, &body);
                        assert!(
                            body.absolute_position().y
                                >= photo.absolute_position().y + photo.size().height + 5.5
                        );
                    }
                    // Photo rows have no heavyweight viewer or scroll area.
                    assert!(
                        photo
                            .query_descendants()
                            .match_type_name("ScrollView")
                            .find_all()
                            .is_empty()
                    );
                    assert_eq!(
                        ElementHandle::find_by_element_type_name(&ui, "PhotoViewer").count(),
                        0
                    );
                }
                let image = ui
                    .get_timeline_rows()
                    .row_data(0)
                    .unwrap()
                    .message
                    .photo_image;
                let edited = Arc::make_mut(&mut view.messages);
                Arc::make_mut(&mut edited[0].content).body =
                    Some("Edited caption 👩🏽‍💻\n\nFull Unicode こんにちは; another line.".into());
                presentation::conversation(&ui, &view, true);
                ui.invoke_layout_timeline();
                assert_eq!(
                    image,
                    ui.get_timeline_rows()
                        .row_data(0)
                        .unwrap()
                        .message
                        .photo_image,
                    "caption reflow must reuse the existing image"
                );
                let item = ElementHandle::find_by_element_type_name(&ui, "TimelineItem")
                    .find(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
                    .unwrap();
                let bubble = child(&item, "MessageBubble::bubble");
                contains(&bubble, &child(&item, "MessageBubble::photo"));
                contains(&bubble, &child(&item, "MessageBubble::body"));
                contains(&bubble, &child(&item, "MessageBubble::stamp"));
            }
        }
    }
    let id = view.messages[0].id;
    ui.set_auto_download_photos(false);
    ui.invoke_request_photos();
    assert!(!demand.borrow().automatic);
    publish(
        &ui,
        &Arc::new(photos::Snapshot {
            rows: [(
                id,
                photos::Photo {
                    state: photos::State::Idle,
                    image: Some(sample.clone()),
                },
            )]
            .into(),
            ..Default::default()
        }),
    );
    let tile = ElementHandle::find_by_element_type_name(&ui, "PhotoTile")
        .find(|e| e.absolute_position().x >= 0.)
        .unwrap();
    let target = tile
        .query_descendants()
        .match_type_name("ContentTarget")
        .find_first()
        .unwrap();
    contains(&tile, &child(&tile, "PhotoTile::status-disc"));
    target.invoke_accessible_default_action();
    assert_eq!(ui.get_photo_viewer_id(), id);
    assert_eq!(demand.borrow().viewer, Some(id));
    let retry = demand.borrow().retry.unwrap();
    assert_eq!(retry.0, id);
    ui.invoke_activate_photo(id);
    assert!(demand.borrow().retry.unwrap().1 > retry.1);
    let full = Arc::new(fixtures::photo_pixels(
        id.unsigned_abs() as usize,
        photos::VIEWER_EDGE,
    ));
    let data = Arc::new(photos::Snapshot {
        rows: [(
            id,
            photos::Photo {
                state: photos::State::Ready,
                image: Some(sample.clone()),
            },
        )]
        .into(),
        viewer: Some((id, full)),
        ..Default::default()
    });
    publish(&ui, &data);
    assert!(ui.get_photo_viewer_ready());
    let viewer = ElementHandle::find_by_element_type_name(&ui, "PhotoViewer")
        .next()
        .unwrap();
    assert_eq!(viewer.size(), ui.window().size().to_logical(1.));
    let zoom = ElementHandle::find_by_accessible_label(&ui, "Zoom in")
        .next()
        .unwrap();
    zoom.invoke_accessible_default_action();
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "Zoom out")
            .next()
            .is_some()
    );
    ElementHandle::find_by_accessible_label(&ui, "Close photo")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(ui.get_photo_viewer_id(), 0);
    assert!(demand.borrow().viewer.is_none());
    assert_eq!(ui.get_photo_viewer_image().size().width, 0);
    ui.invoke_activate_photo(id);
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
    assert_eq!(ui.get_photo_viewer_id(), 0);
    ui.invoke_activate_photo(id);
    view.messages = Arc::from([]);
    presentation::conversation(&ui, &view, false);
    assert_eq!(
        ui.get_photo_viewer_id(),
        id,
        "paging cannot close an album photo"
    );
    publish(
        &ui,
        &Arc::new(photos::Snapshot {
            checked: Some((9, id)),
            ..Default::default()
        }),
    );
    assert_eq!(ui.get_photo_viewer_id(), 0);
    assert!(demand.borrow().ids().is_empty());
    ui.invoke_activate_photo(id);
    assert_eq!(ui.get_photo_viewer_id(), 0, "removed message cannot reopen");

    check_album(&ui, &messages[0], &sample);

    let mut many = (0..10000)
        .map(|i| {
            let mut m = messages[0].clone();
            m.id = 30000 + i;
            m
        })
        .collect::<Vec<_>>();
    fixtures::fill_photos(&mut many);
    view.messages = many.into();
    presentation::conversation(&ui, &view, true);
    for width in [1200., 520., 800., 1920., 800.] {
        ui.window().set_size(slint::LogicalSize::new(width, 900.));
        for y in [0., -10000., -50000.] {
            ui.set_timeline_y(y);
            ui.invoke_layout_timeline();
            assert!(ui.get_timeline_rows().row_count() < 30);
            assert!(demand.borrow().ids().len() <= photos::MAX_DEMAND + 1);
        }
    }
    check_wheel_motion(&ui, &view, &sample);
}

fn check_grid() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let demand = bind(&ui);
    let mut demo = fixtures::demo();
    demo.select(9);
    let mut view = demo.conversation();
    let seed = view.messages[0].clone();
    let pixels = Arc::new(fixtures::photo_pixels(0, photos::INLINE_EDGE));
    for count in [2, 3, 4, 37, 38, 1000] {
        view.messages = if count == 37 {
            fixtures::unlinked_album_messages(&seed, count)
        } else {
            fixtures::album_messages(&seed, count)
        }
        .into();
        if count == 4 {
            let messages = Arc::make_mut(&mut view.messages);
            for message in messages.iter_mut() {
                message.outgoing = true;
                message.time = format!("10:56 · {}", "Synthetic delivery status ".repeat(6));
            }
            let content = Arc::make_mut(&mut messages[0].content);
            content.body = Some("Album caption 👩🏽‍💻\nA second line stays visible.".into());
            content.quote = Some(crate::message_content::Quote {
                sender: "Sample".into(),
                text: "An album reply".into(),
            });
            content.reaction_summary = "❤️ 2".into();
        }
        view.transcript = view
            .messages
            .iter()
            .map(|m| m.transcript_line())
            .collect::<Vec<_>>()
            .join("\n")
            .into();
        for dark in [false, true] {
            ui.global::<Theme>().set_dark(dark);
            for (width, height) in [(1200., 900.), (800., 720.), (520., 720.)] {
                ui.window().set_size(slint::LogicalSize::new(width, height));
                presentation::conversation(&ui, &view, true);
                ui.show().unwrap();
                ui.invoke_layout_timeline();
                assert_eq!(ui.get_messages().row_count(), count);
                assert_eq!(ui.get_timeline_rows().row_count(), 1);
                let row = ui.get_timeline_rows().row_data(0).unwrap();
                assert_eq!(row.message.album_photos.row_count(), count.min(4));
                assert_eq!(row.message.album_more, count.saturating_sub(4) as i32);
                if count == 4 {
                    assert_eq!(
                        row.message.text,
                        view.messages[0].content.body.as_deref().unwrap()
                    );
                    assert_eq!(row.message.quote_text, "An album reply");
                    assert_eq!(row.message.reactions, "❤️ 2");
                }
                assert_eq!(
                    demand.borrow().visible,
                    (44000..44000 + count.min(4) as i32).collect::<Vec<_>>()
                );
                let before = (ui.get_timeline_height(), ui.get_timeline_y());
                let mut data = Arc::new(photos::Snapshot {
                    rows: (44000..44000 + count.min(4) as i32)
                        .map(|id| {
                            (
                                id,
                                photos::Photo {
                                    state: photos::State::Loading,
                                    image: Some(pixels.clone()),
                                },
                            )
                        })
                        .collect(),
                    ..Default::default()
                });
                publish(&ui, &data);
                assert_eq!(
                    before,
                    (ui.get_timeline_height(), ui.get_timeline_y()),
                    "previews do not reflow grids"
                );
                let item = ElementHandle::find_by_element_type_name(&ui, "TimelineItem")
                    .find(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
                    .unwrap();
                let grid = item
                    .query_descendants()
                    .match_type_name("PhotoGrid")
                    .find_first()
                    .unwrap();
                contains(&child(&item, "MessageBubble::bubble"), &grid);
                if count == 4 {
                    let bubble = child(&item, "MessageBubble::bubble");
                    let body = child(&item, "MessageBubble::body");
                    contains(&bubble, &body);
                    assert!(
                        body.absolute_position().y
                            >= grid.absolute_position().y + grid.size().height
                    );
                }
                let tiles = grid
                    .query_descendants()
                    .match_type_name("PhotoTile")
                    .find_all();
                assert_eq!(tiles.len(), count.min(4));
                for (index, tile) in tiles.iter().enumerate() {
                    contains(&grid, tile);
                    assert_eq!(
                        tile.query_descendants()
                            .match_id("PhotoTile::status-disc")
                            .find_all()
                            .len(),
                        0,
                        "album photos load without overlays"
                    );
                    let p = tile.absolute_position();
                    let size = tile.size();
                    let id = crate::presentation::timeline::context_target(
                        &ui,
                        p.x + size.width / 2.,
                        p.y + size.height / 2.,
                    );
                    assert_eq!(
                        id,
                        Some(44000 + index as i32),
                        "native context menus target the clicked photo"
                    );
                }
                for photo in Arc::make_mut(&mut data).rows.values_mut() {
                    photo.state = photos::State::Ready;
                }
                publish(&ui, &data);
                assert_eq!(before, (ui.get_timeline_height(), ui.get_timeline_y()));
                assert!(crate::presentation::timeline::navigate(
                    &ui,
                    44000 + count as i32 - 1,
                    false
                ));
                assert_eq!(
                    crate::presentation::timeline::display_id(&ui, 44000 + count as i32 - 1),
                    44000
                );
            }
        }
    }
    view.messages = fixtures::album_messages(&seed, 38).into();
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    presentation::conversation(&ui, &view, true);
    ui.invoke_layout_timeline();
    let compact_height = ui.get_timeline_height();
    ui.set_selection_active(true);
    ui.invoke_layout_timeline();
    assert!(
        ui.get_timeline_height() > compact_height * 3.,
        "selection exposes the originals"
    );
    assert_eq!(
        crate::presentation::timeline::selected_ids(&ui, &[44037].into()),
        [44037]
    );
    ui.set_selection_active(false);
    ui.invoke_layout_timeline();
    assert!((ui.get_timeline_height() - compact_height).abs() < 1.);
    let item = ElementHandle::find_by_element_type_name(&ui, "TimelineItem")
        .find(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
        .unwrap();
    let tiles = child(&item, "MessageBubble::bubble")
        .query_descendants()
        .match_type_name("PhotoTile")
        .find_all();
    let position = tiles[3].absolute_position();
    let size = tiles[3].size();
    let position =
        slint::LogicalPosition::new(position.x + size.width / 2., position.y + size.height / 2.);
    let clicked = Rc::new(Cell::new((0, -1)));
    let recorded = clicked.clone();
    ui.on_message_action(move |id, action, _, _| recorded.set((id, action)));
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
    child(&item, "MessageHoverControls::forward").invoke_accessible_default_action();
    assert_eq!(
        clicked.get(),
        (44003, 2),
        "hover actions retain the hovered photo's identity"
    );
    for pressed in [true, false] {
        ui.window().dispatch_event(if pressed {
            slint::platform::WindowEvent::PointerPressed {
                position,
                button: slint::platform::PointerEventButton::Left,
            }
        } else {
            slint::platform::WindowEvent::PointerReleased {
                position,
                button: slint::platform::PointerEventButton::Left,
            }
        });
    }
    assert_eq!(
        ui.get_photo_viewer_id(),
        44003,
        "the actual counter tile receives pointer clicks"
    );
    ui.invoke_close_photo();
    tiles[3]
        .query_descendants()
        .match_type_name("ContentTarget")
        .find_first()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(
        ui.get_photo_viewer_id(),
        44003,
        "the +N tile opens its photo in the full viewer"
    );
    assert_eq!(demand.borrow().viewer, Some(44003));
    let members = (44000..44038).collect::<Vec<_>>();
    publish(
        &ui,
        &Arc::new(photos::Snapshot {
            checked: Some((9, 44003)),
            gallery: Some(Arc::new(photos::Gallery {
                id: 44003,
                members,
                position: 3,
                sender: "Mira Sample".into(),
                date: "Today".into(),
                caption: String::new(),
                message: None,
            })),
            viewer: Some((44003, pixels.clone())),
            ..Default::default()
        }),
    );
    assert_eq!(ui.get_photo_gallery_ids().row_count(), 38);
    assert_eq!(ui.get_photo_viewer_next(), 44004);
    ui.invoke_navigate_photo(44004);
    assert_eq!(demand.borrow().viewer, Some(44004));
    ui.invoke_close_photo();
    assert_eq!(ui.get_photo_viewer_id(), 0);
    // The same album can straddle a cached page; don't use the page size as
    // the count, or request originals for its off-page members.
    view.messages = view.messages[36..].to_vec().into();
    presentation::conversation(&ui, &view, true);
    ui.invoke_layout_timeline();
    let row = ui.get_timeline_rows().row_data(0).unwrap();
    assert_eq!(row.message.album_photos.row_count(), 2);
    assert_eq!(row.message.album_more, 36);
    assert_eq!(demand.borrow().visible, [44036, 44037]);
    let item = ElementHandle::find_by_element_type_name(&ui, "TimelineItem")
        .find(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
        .unwrap();
    let download_icons = || {
        item.query_descendants()
            .match_id("PhotoTile::status-disc")
            .find_all()
            .len()
    };
    ui.set_auto_download_photos(false);
    assert_eq!(
        download_icons(),
        1,
        "manual download remains available; +N keeps its own affordance"
    );
    ui.set_auto_download_photos(true);
    assert_eq!(download_icons(), 0, "automatic loading stays unobstructed");
}

fn check_wheel_motion(
    _ui: &AppWindow,
    view: &crate::domain::Conversation,
    pixels: &Arc<photos::Pixels>,
) {
    let view = view.clone();
    let pixels = pixels.clone();
    // The surrounding service suite needs system time. Give this finite check
    // its own toolkit thread and mock clock so a busy CI runner cannot sleep
    // through the whole 180 ms animation or advance the suite's clock.
    std::thread::spawn(move || {
        use std::time::Duration;
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.set_demo_mode(true);
        ui.set_show_conversation(true);
        let _demand = bind(&ui);
        ui.window().set_size(slint::LogicalSize::new(1200., 900.));
        presentation::conversation(&ui, &view, true);
        ui.set_timeline_y(0.);
        ui.invoke_layout_timeline();
        // At the top these photo rows have already been measured. Scroll within
        // them so changes to the estimated height of distant history are irrelevant.
        ui.set_timeline_y(-200.);
        ui.invoke_layout_timeline();
        let start = ui.get_timeline_y();
        let data = Arc::new(photos::Snapshot {
            rows: ui
                .get_timeline_rows()
                .iter()
                .map(|row| {
                    (
                        row.message.id,
                        photos::Photo {
                            state: photos::State::Ready,
                            image: Some(pixels.clone()),
                        },
                    )
                })
                .collect(),
            viewer: None,
            ..Default::default()
        });
        let wheel = |delta| {
            crate::presentation_scroll::dispatch_wheel(
                ui.window(),
                slint::LogicalPosition::new(800., 350.),
                0.,
                delta,
            );
        };
        let frame = || {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(16));
            // Texture arrival and an unrelated snapshot must preserve the animation.
            publish(&ui, &data);
            presentation::conversation(&ui, &view, false);
            ui.invoke_layout_timeline();
            ui.get_timeline_y()
        };
        wheel(-120.);
        assert!(
            (ui.get_timeline_y() - start).abs() < 1.,
            "wheel must animate instead of jumping"
        );
        let positions = (0..17).map(|_| frame()).collect::<Vec<_>>();
        assert!(
            (ui.get_timeline_y() - start + 120.).abs() < 1.,
            "one wheel notch: {start} -> {positions:?}"
        );
        assert!(
            positions.windows(2).all(|p| p[1] <= p[0] + 0.1),
            "no backwards jumps: {positions:?}"
        );
        assert!(
            positions
                .iter()
                .any(|p| *p < start - 1. && *p > start - 119.),
            "intermediate animation positions exist: {positions:?}"
        );
        wheel(120.);
        for _ in 0..17 {
            frame();
        }
        assert!((ui.get_timeline_y() - start).abs() < 1.);

        // Notches arriving before the previous animation finishes retain their
        // combined distance, rather than dropping or doubling the remaining motion.
        let mut positions = Vec::new();
        for _ in 0..3 {
            let before = ui.get_timeline_y();
            wheel(-120.);
            assert!((ui.get_timeline_y() - before).abs() < 1.);
            positions.push(frame());
        }
        positions.extend((0..17).map(|_| frame()));
        assert!(
            (ui.get_timeline_y() - start + 360.).abs() < 1.,
            "three rapid wheel notches: {start} -> {positions:?}"
        );
        assert!(
            positions.windows(2).all(|p| p[1] <= p[0] + 0.1),
            "no backwards jumps during a wheel burst: {positions:?}"
        );
    })
    .join()
    .unwrap();
}

fn check_album(ui: &AppWindow, message: &crate::domain::Message, pixels: &Arc<photos::Pixels>) {
    let mut demo = fixtures::demo();
    demo.select(9);
    let mut view = demo.conversation();
    view.messages = Arc::from([message.clone()]);
    presentation::conversation(ui, &view, true);
    ui.invoke_layout_timeline();
    ui.invoke_activate_photo(message.id);
    let members = std::iter::once(message.id)
        .chain(70001..70020)
        .collect::<Vec<_>>();
    let data = |id: i32| {
        let position = members.iter().position(|&m| m == id).unwrap();
        let gallery = photos::Gallery {
            id,
            members: members.clone(),
            position,
            sender: "~Mira Sample".into(),
            date: "21 Oct 10:24".into(),
            caption: "A long caption 👩🏽‍💻\nSecond line مرحبا · こんにちは\n".repeat(20),
            message: None,
        };
        Arc::new(photos::Snapshot {
            checked: Some((9, id)),
            gallery: Some(Arc::new(gallery)),
            viewer: Some((id, pixels.clone())),
            ..Default::default()
        })
    };
    publish(ui, &data(message.id));
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for (width, height) in [(520., 500.), (800., 600.), (1920., 1080.)] {
            ui.window().set_size(slint::LogicalSize::new(width, height));
            let viewer = ElementHandle::find_by_element_type_name(ui, "PhotoViewer")
                .next()
                .unwrap();
            for id in [
                "PhotoViewer::header",
                "PhotoViewer::stage",
                "PhotoViewer::footer",
                "PhotoViewer::strip",
            ] {
                contains(&viewer, &child(&viewer, id));
            }
            let header = child(&viewer, "PhotoViewer::header");
            let stage = child(&viewer, "PhotoViewer::stage");
            let footer = child(&viewer, "PhotoViewer::footer");
            assert!((header.absolute_position().y - viewer.absolute_position().y).abs() < 0.5);
            assert!(
                stage.absolute_position().y
                    >= header.absolute_position().y + header.size().height - 0.5
            );
            assert!(
                footer.absolute_position().y
                    >= stage.absolute_position().y + stage.size().height - 0.5
            );
            let caption = child(&viewer, "PhotoViewer::caption-scroll");
            let strip = child(&viewer, "PhotoViewer::strip");
            assert!(
                strip.absolute_position().y
                    >= caption.absolute_position().y + caption.size().height + 7.5
            );
            let thumbs = strip
                .query_descendants()
                .match_type_name("Rectangle")
                .find_all();
            for thumb in thumbs
                .iter()
                .filter(|t| t.id().as_deref() == Some("PhotoViewer::thumb"))
            {
                contains(&strip, thumb);
            }
            assert_eq!(ui.get_photo_thumbnails().row_count(), photos::THUMB_COUNT);
        }
    }
    assert_eq!(ui.get_photo_viewer_previous(), 0);
    let next = ElementHandle::find_by_accessible_label(ui, "Next photo")
        .next()
        .unwrap();
    next.invoke_accessible_default_action();
    assert_eq!(
        ui.get_photo_viewer_id(),
        70001,
        "navigation reaches photos outside the timeline"
    );
    assert!(!ui.get_photo_viewer_ready());
    assert_eq!(
        ui.get_photo_viewer_image().size().width,
        0,
        "no previous full image while opening the next"
    );
    publish(ui, &data(70001));
    let ready = ui.get_photo_viewer_image();
    publish(ui, &data(message.id));
    assert_eq!(
        ui.get_photo_viewer_image(),
        ready,
        "late publication cannot replace/clear the new photo"
    );
    ui.invoke_navigate_photo(*members.last().unwrap());
    publish(ui, &data(*members.last().unwrap()));
    assert_eq!(ui.get_photo_viewer_next(), 0);
    let zoom = ElementHandle::find_by_accessible_label(ui, "Zoom in")
        .next()
        .unwrap();
    zoom.invoke_accessible_default_action();
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::LeftArrow.into(),
        });
    assert_eq!(ui.get_photo_viewer_id(), members[members.len() - 2]);
    assert!(
        ElementHandle::find_by_accessible_label(ui, "Zoom in")
            .next()
            .is_some(),
        "changing photos resets zoom"
    );
    let id = ui.get_photo_viewer_id();
    publish(
        ui,
        &Arc::new(photos::Snapshot {
            checked: Some((9, id)),
            ..Default::default()
        }),
    );
    assert_eq!(
        ui.get_photo_viewer_id(),
        0,
        "deletion/expiry/locking closes an invalid selection"
    );
    assert_eq!(ui.get_photo_thumbnails().row_count(), 0);
    assert_eq!(ui.get_photo_gallery_ids().row_count(), 0);
}
