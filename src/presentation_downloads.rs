//! One window-level transfer panel; no file/network work on the UI thread.
use crate::{
    AppWindow,
    archive::Archive,
    downloads::{self, Command, Kind, Phase, Status, Target},
};
use slint::{
    ComponentHandle, Model,
    winit_031::{
        WinitWindowAccessor,
        winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
    },
};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

pub(crate) fn parent(ui: &AppWindow) -> String {
    ui.window()
        .with_winit_window(
            |window| match window.window_handle().ok().map(|h| h.as_raw()) {
                Some(RawWindowHandle::Xlib(h)) => format!("x11:{:x}", h.window),
                Some(RawWindowHandle::Xcb(h)) => format!("x11:{:x}", h.window.get()),
                // The portal accepts an empty identifier. Wayland export/parenting is
                // separate from choosing the file; no raw pointer is kept after here.
                _ => String::new(),
            },
        )
        .unwrap_or_default()
}
pub(crate) fn bind(ui: &AppWindow, commands: mpsc::Sender<Command>) {
    let weak = ui.as_weak();
    let tx = commands.clone();
    ui.on_download_message(move |chat, message| {
        let Some(ui) = weak.upgrade() else { return };
        if chat != ui.get_selected_id()
            || (!ui.get_demo_mode() && ui.get_live_revision() != ui.get_live_shown_revision())
        {
            return;
        }
        let Some(row) = ui
            .get_timeline_rows()
            .iter()
            .find(|r| r.message.id == message && r.message.has_attachment && r.message.actionable)
        else {
            return;
        };
        let demo = ui.get_demo_mode().then_some(if row.message.has_photo {
            Kind::Photo
        } else if row.message.has_audio {
            Kind::Audio
        } else {
            Kind::Document
        });
        if tx
            .try_send(Command::Start(Target {
                chat,
                message,
                parent: parent(&ui),
                demo,
            }))
            .is_err()
        {
            ui.set_notice("Downloads are busy. Please try again.".into());
        }
    });
    let weak = ui.as_weak();
    ui.on_download_control(move |action| {
        let command = match action {
            0 => Command::Cancel,
            1 => Command::Retry,
            2 => Command::Dismiss,
            _ => return,
        };
        if commands.try_send(command).is_err()
            && let Some(ui) = weak.upgrade()
        {
            ui.set_notice("Download controls are busy. Please try again.".into());
        }
    });
}
fn bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.0} KiB", n as f64 / 1024.)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", n as f64 / (1024. * 1024.))
    } else {
        format!("{:.2} GiB", n as f64 / (1024. * 1024. * 1024.))
    }
}
pub fn publish(ui: &AppWindow, status: &Status) {
    ui.set_download_phase(status.phase as i32);
    ui.set_download_name(status.name.as_str().into());
    ui.set_download_fraction(status.total.filter(|n| *n > 0).map_or(-1., |n| {
        (status.bytes as f64 / n as f64).clamp(0., 1.) as f32
    }));
    ui.set_download_status(match status.phase {
        Phase::Choosing => "Choose where to save the attachment…".into(),
        Phase::Downloading => status
            .total
            .map_or_else(
                || format!("Downloading · {}", bytes(status.bytes)),
                |total| format!("Downloading · {} of {}", bytes(status.bytes), bytes(total)),
            )
            .into(),
        Phase::Committing => "Saving verified attachment…".into(),
        Phase::Saved => "Saved".into(),
        _ => status.error.map_or("", |e| e.label()).into(),
    });
}
pub fn attach(
    ui: &AppWindow,
    archives: watch::Receiver<Option<Archive>>,
    sources: watch::Receiver<downloads::Sources>,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) -> impl std::future::Future<Output = ()> + Send + use<> {
    let (commands, rx) = mpsc::channel(8);
    bind(ui, commands);
    let (updates, snapshots) = watch::channel(Status::default());
    let weak = ui.as_weak();
    async move {
        tokio::join!(
            downloads::run(
                archives,
                sources,
                rx,
                updates,
                Arc::new(crate::downloads_picker::NativePicker(preferences))
            ),
            crate::live_presentation::present_latest(weak, snapshots, publish)
        );
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
    let (tx, mut commands) = mpsc::channel(8);
    bind(&ui, tx);
    crate::presentation_actions::bind(&ui, |_| {
        panic!("Download must not enqueue a message operation")
    });
    ui.show().unwrap();
    ui.invoke_layout_timeline();
    let id = ui
        .get_timeline_rows()
        .iter()
        .find(|r| r.message.has_photo)
        .unwrap()
        .message
        .id;
    ui.invoke_message_action(id, 3, 500., 300.);
    ui.invoke_choose_message_menu(5);
    assert!(
        matches!(commands.try_recv().unwrap(),Command::Start(Target {chat:9,message,demo:Some(Kind::Photo),..}) if message==id)
    );
    assert_eq!(ui.get_message_dialog(), 0);
    assert!(commands.try_recv().is_err());
    let rows = ui.get_timeline_rows();
    let geometry = (ui.get_timeline_y(), ui.get_timeline_height());
    let mut status = Status {
        phase: Phase::Downloading,
        name: "Synthetic long filename 你好.pdf".into(),
        total: Some(1024 * 1024),
        ..Default::default()
    };
    publish(&ui, &status);
    let track = ElementHandle::find_by_element_id(&ui, "DownloadStatus::track")
        .next()
        .unwrap();
    let fill = ElementHandle::find_by_element_id(&ui, "DownloadStatus::fill")
        .next()
        .unwrap();
    assert_eq!(fill.size().width, 0.);
    status.bytes = 512 * 1024;
    publish(&ui, &status);
    assert_eq!(fill.absolute_position(), track.absolute_position());
    assert!((fill.size().width - track.size().width / 2.).abs() < 0.1);
    crate::presentation::timeline::take_samples();
    for bytes in 0..1000 {
        status.bytes = bytes * 1024;
        publish(&ui, &status);
    }
    assert_eq!(ui.get_timeline_rows(), rows);
    assert_eq!((ui.get_timeline_y(), ui.get_timeline_height()), geometry);
    assert!(
        crate::presentation::timeline::take_samples()
            .milliseconds
            .is_empty(),
        "transfer progress cannot rebuild or lay out the timeline"
    );
    for (width, height) in [(1200., 800.), (800., 600.), (520., 360.)] {
        ui.window().set_size(slint::LogicalSize::new(width, height));
        let panel = ElementHandle::find_by_element_id(&ui, "AppWindow::download")
            .next()
            .unwrap();
        let p = panel.absolute_position();
        let size = panel.size();
        assert!(
            p.x >= 7.5
                && p.y >= 7.5
                && p.x + size.width <= width - 7.5
                && p.y + size.height <= height - 7.5
        );
        assert_eq!(
            ElementHandle::find_by_element_type_name(&ui, "DownloadStatus").count(),
            1
        );
    }
    ui.invoke_message_action(id, 3, 350., 100.);
    assert!(
        ui.get_message_menu_entries()
            .iter()
            .any(|e| e.action == 5 && !e.enabled)
    );
    ui.invoke_choose_message_menu(5);
    assert!(commands.try_recv().is_err());
    ui.invoke_close_message_actions();
    ElementHandle::find_by_accessible_label(&ui, "Cancel download")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(matches!(commands.try_recv().unwrap(), Command::Cancel));
    status.phase = Phase::Failed;
    status.error = Some(downloads::Error::Network);
    publish(&ui, &status);
    ElementHandle::find_by_accessible_label(&ui, "Retry download")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(matches!(commands.try_recv().unwrap(), Command::Retry));
    ElementHandle::find_by_accessible_label(&ui, "Dismiss download")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(matches!(commands.try_recv().unwrap(), Command::Dismiss));
    publish(&ui, &Status::default());
    assert_eq!(
        ElementHandle::find_by_element_type_name(&ui, "DownloadStatus").count(),
        0
    );
    ui.hide().unwrap();
}
