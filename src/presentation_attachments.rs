//! A single bounded review overlay; progress never touches the chat model.
use crate::{
    AppWindow,
    archive::Archive,
    attachments::{self, Command, Snapshot},
};
use slint::{ComponentHandle, Model};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
pub fn attach(
    ui: &AppWindow,
    archives: watch::Receiver<Option<Archive>>,
    sources: watch::Receiver<attachments::Sources>,
    demo: bool,
) -> (
    mpsc::Sender<Command>,
    impl std::future::Future<Output = ()> + Send + use<>,
) {
    let (tx, rx) = mpsc::channel(32);
    bind(ui, tx.clone());
    let clipboard = crate::presentation_clipboard::attach(ui, archives.clone(), tx.clone(), demo);
    let (updates, snapshots) = watch::channel(Snapshot::default());
    let weak = ui.as_weak();
    (tx, async move {
        tokio::join!(
            clipboard,
            attachments::run(
                archives,
                sources,
                rx,
                updates,
                Arc::new(attachments::NativePicker),
                demo
            ),
            crate::live_presentation::present_latest(weak, snapshots, publish)
        );
    })
}
fn bind(ui: &AppWindow, tx: mpsc::Sender<Command>) {
    let weak = ui.as_weak();
    let picks = tx.clone();
    ui.on_pick_attachments(move |kind| {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_clipboard_busy()
            || ui.get_voice_phase() > 0
            || ui.get_selected_id() <= 0
            || ui.get_attachment_phase() != 0
            || (!ui.get_demo_mode()
                && (ui.get_connection_state() != 5
                    || !ui.get_live_can_send()
                    || ui.get_live_revision() != ui.get_live_shown_revision()))
        {
            return;
        }
        if picks
            .try_send(Command::Pick {
                chat: ui.get_selected_id(),
                title: ui.get_chat_title().into(),
                parent: crate::presentation_downloads::parent(&ui),
                kind: if kind == 0 {
                    attachments::Kind::Photo
                } else {
                    attachments::Kind::Document
                },
            })
            .is_err()
        {
            ui.set_notice("Attachment selection is busy. Please try again.".into());
        }
    });
    let weak = ui.as_weak();
    ui.on_attachment_control(move |action, index, caption| {
        let Some(ui) = weak.upgrade() else { return };
        let command = match action {
            0 => Command::Cancel,
            1 => Command::Send(caption.into()),
            2 => Command::Select(index.max(0) as usize, caption.into()),
            3 => Command::Remove(caption.into()),
            _ => return,
        };
        if tx.try_send(command).is_err() {
            ui.set_attachment_status(
                "Heya is busy. Please try again; your caption has been kept.".into(),
            );
        }
    });
}
pub fn publish(ui: &AppWindow, s: &Snapshot) {
    ui.set_attachment_voice(s.voice);
    let restore_focus = ui.get_attachment_review_open();
    if matches!(s.phase, 1..=3) {
        ui.set_attachment_review_open(true);
    } else if matches!(s.phase, 0 | 4 | 5) {
        ui.set_attachment_review_open(false);
    }
    let old = ui.get_attachment_items();
    if old.row_count() != s.items.len()
        || !old
            .iter()
            .zip(s.items.iter())
            .all(|(a, b)| a.key.as_str() == b.id)
    {
        let rows = s
            .items
            .iter()
            .map(|item| {
                let preview = old
                    .iter()
                    .find(|row| row.key.as_str() == item.id)
                    .map(|r| r.preview)
                    .unwrap_or_else(|| {
                        item.preview
                            .as_ref()
                            .map_or_else(slint::Image::default, |p| {
                                slint::Image::from_rgba8(
                                    slint::SharedPixelBuffer::clone_from_slice(
                                        &p.rgba, p.width, p.height,
                                    ),
                                )
                            })
                    });
                crate::ui::AttachmentItem {
                    key: item.id.as_str().into(),
                    name: item.name.as_str().into(),
                    size: size(item.size).into(),
                    preview,
                }
            })
            .collect::<Vec<_>>();
        ui.set_attachment_items(slint::ModelRc::new(slint::VecModel::from(rows)));
    }
    ui.set_attachment_title(s.title.as_str().into());
    ui.set_attachment_selected(s.selected as i32);
    ui.set_attachment_caption(s.caption.as_str().into());
    ui.set_attachment_progress(if s.total == 0 {
        0.
    } else {
        (s.bytes as f64 / s.total as f64).clamp(0., 1.) as f32
    });
    ui.set_attachment_status(
        if !s.error.is_empty() {
            s.error.clone()
        } else {
            match s.phase {
                1 => "Choose files in the native dialog…".into(),
                2 => "Preparing encrypted attachments…".into(),
                3 => format!(
                    "{} attachment(s). Review the recipient and captions before sending.",
                    s.items.len()
                ),
                4 => format!(
                    "{} · {} of {}",
                    s.items.first().map_or("Attachment", |f| f.name.as_str()),
                    size(s.bytes),
                    size(s.total)
                ),
                5 => "Saving to the encrypted outbox…".into(),
                _ => String::new(),
            }
        }
        .into(),
    );
    ui.set_attachment_phase(s.phase);
    if s.phase == 0 && restore_focus {
        ui.invoke_focus_composer();
    }
}
fn size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KiB", n as f64 / 1024.)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", n as f64 / 1048576.)
    } else {
        format!("{:.2} GiB", n as f64 / 1073741824.)
    }
}

#[cfg(test)]
pub(crate) fn check_ui() {
    use i_slint_backend_testing::ElementHandle;
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let mut demo = crate::fixtures::demo();
    demo.select(9);
    crate::presentation::conversation(&ui, &demo.conversation(), true);
    ui.set_draft("Keep this draft".into());
    let (tx, mut commands) = mpsc::channel(32);
    bind(&ui, tx);
    ui.show().unwrap();
    ui.set_attachment_menu(true);
    ui.set_attachment_x(330.);
    ui.set_attachment_y(700.);
    ElementHandle::find_by_accessible_label(&ui, "Photos")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::Pick {
            chat: 9,
            kind: attachments::Kind::Photo,
            ..
        }
    ));
    assert!(!ui.get_attachment_menu());
    let pixels = Arc::new(crate::photos::Pixels {
        width: 2,
        height: 2,
        rgba: vec![100; 16],
    });
    let mut s = Snapshot {
        phase: 3,
        title: "Synthetic recipient".into(),
        items: vec![
            attachments::Item {
                id: "first".into(),
                name: "Unicode 你好.png".into(),
                size: 100,
                preview: Some(pixels),
            },
            attachments::Item {
                id: "second".into(),
                name: "Very long document name.pdf".into(),
                size: 500,
                preview: None,
            },
        ]
        .into(),
        ..Default::default()
    };
    publish(&ui, &s);
    let model = ui.get_attachment_items();
    let image = model.row_data(0).unwrap().preview;
    let timeline = ui.get_timeline_rows();
    crate::presentation::timeline::take_samples();
    s.phase = 4;
    s.total = 1000;
    publish(&ui, &s);
    crate::presentation::timeline::take_samples();
    for i in 0..1000 {
        s.bytes = i;
        publish(&ui, &s);
    }
    assert_eq!(model, ui.get_attachment_items());
    assert_eq!(
        image,
        ui.get_attachment_items().row_data(0).unwrap().preview
    );
    assert_eq!(timeline, ui.get_timeline_rows());
    assert!(
        crate::presentation::timeline::take_samples()
            .milliseconds
            .is_empty()
    );
    assert_eq!(ui.get_draft(), "Keep this draft");
    assert_eq!(
        ElementHandle::find_by_element_type_name(&ui, "AttachmentReview").count(),
        0
    );
    assert!(
        ui.get_history_active(),
        "background uploads cannot block chat scrolling"
    );
    s.phase = 3;
    publish(&ui, &s);
    for (width, height) in [(1200., 800.), (800., 600.), (520., 480.), (520., 360.)] {
        ui.window().set_size(slint::LogicalSize::new(width, height));
        let panel = ElementHandle::find_by_element_id(&ui, "AttachmentReview::panel")
            .next()
            .unwrap();
        let p = panel.absolute_position();
        let size = panel.size();
        assert!(
            p.x >= 11.5
                && p.y >= 11.5
                && p.x + size.width <= width - 11.5
                && p.y + size.height <= height - 11.5
        );
        for label in ["Remove attachment", "Send", "Close attachments"] {
            let e = ElementHandle::find_by_accessible_label(&ui, label).next();
            if let Some(e) = e {
                let p = e.absolute_position();
                let size = e.size();
                assert!(
                    p.y >= 0. && p.y + size.height <= height,
                    "{label} outside {height}"
                );
            }
        }
    }
    s.phase = 3;
    s.caption = "Caption".into();
    publish(&ui, &s);
    ui.set_demo_mode(false);
    ui.set_connection_state(5);
    ElementHandle::find_by_accessible_label(&ui, "Send")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(matches!(commands.try_recv().unwrap(),Command::Send(c) if c=="Caption"));
    ElementHandle::find_by_accessible_label(&ui, "Remove attachment")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(matches!(commands.try_recv().unwrap(), Command::Remove(_)));
    publish(&ui, &Snapshot::default());
    assert_eq!(
        ElementHandle::find_by_element_type_name(&ui, "AttachmentReview").count(),
        0
    );
    assert_eq!(ui.get_attachment_items().row_count(), 0);
    ui.hide().unwrap();
}
