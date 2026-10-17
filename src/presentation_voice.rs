//! One composer strip; capture/playback progress never replaces timeline rows.
use crate::{
    AppWindow,
    archive::Archive,
    attachments,
    voice::{self, Command, Status},
};
use slint::ComponentHandle;
use tokio::sync::{mpsc, watch};
pub fn attach(
    ui: &AppWindow,
    archives: watch::Receiver<Option<Archive>>,
    sources: watch::Receiver<attachments::Sources>,
    uploads: mpsc::Sender<attachments::Command>,
    demo: bool,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) -> impl std::future::Future<Output = ()> + Send + use<> {
    let (commands, rx) = mpsc::channel(16);
    bind(ui, commands);
    let (updates, snapshots) = watch::channel(Status::default());
    let weak = ui.as_weak();
    async move {
        tokio::join!(
            voice::run(archives, sources, rx, updates, uploads, demo, preferences),
            crate::live_presentation::present_latest(weak, snapshots, publish)
        );
    }
}
fn bind(ui: &AppWindow, commands: mpsc::Sender<Command>) {
    let weak = ui.as_weak();
    ui.set_voice_supported(cfg!(all(feature = "media-gstreamer", target_os = "linux")));
    ui.on_voice_control(move |action| {
        let Some(ui) = weak.upgrade() else { return };
        let command = match action {
            0 if ui.get_voice_phase() == 0
                && !ui.get_microphone_dialog()
                && ui.get_attachment_phase() == 0
                && ui.get_selected_id() > 0
                && (ui.get_demo_mode()
                    || (ui.get_connection_state() == 5
                        && ui.get_live_can_send()
                        && ui.get_live_revision() == ui.get_live_shown_revision())) =>
            {
                ui.invoke_audio_control(0, 3, 0.);
                Command::Start {
                    chat: ui.get_selected_id(),
                    title: ui.get_chat_title().into(),
                }
            }
            1 => Command::Cancel,
            2 => Command::Pause,
            3 => Command::Finish,
            4 => Command::Preview,
            5 => Command::Send,
            6 => Command::ClearReply,
            _ => return,
        };
        if commands.try_send(command).is_err() {
            ui.set_notice("Voice recording is busy. Please try again.".into());
        } else if action == 0 {
            ui.set_voice_phase(6);
        }
    });
}
pub fn publish(ui: &AppWindow, s: &Status) {
    let before = ui.get_voice_phase();
    ui.set_voice_phase(s.phase);
    ui.set_voice_playing(s.playing);
    ui.set_voice_time(
        crate::presentation_audio::duration(if s.playing { s.position } else { s.seconds }).into(),
    );
    let path = crate::presentation_audio::waveform(&s.levels);
    if ui.get_voice_waveform().as_str() != path {
        ui.set_voice_waveform(path.into());
    }
    ui.set_voice_status(
        if !s.error.is_empty() {
            s.error.as_str()
        } else {
            match s.phase {
                1 => "Recording",
                2 => "Paused",
                3 => "Finishing recording…",
                4 => "Review your voice message",
                5 => "Preparing to send…",
                6 => "Checking for a microphone…",
                _ => "",
            }
        }
        .into(),
    );
    if s.phase == 0 {
        if s.failure == Some(crate::voice_recorder::Error::NoInput) {
            ui.set_microphone_dialog(true);
        } else if !s.error.is_empty() {
            ui.set_notice(s.error.as_str().into());
        }
        if before > 0 {
            ui.invoke_focus_composer();
        }
    }
}

#[cfg(test)]
pub(crate) fn check_ui() {
    use i_slint_backend_testing::ElementHandle;
    use slint::Model;
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let mut state = crate::fixtures::demo();
    state.select(10);
    crate::presentation::conversation(&ui, &state.conversation(), true);
    let (tx, mut rx) = mpsc::channel(16);
    bind(&ui, tx);
    ui.set_draft("".into());
    ui.show().unwrap();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    if cfg!(all(feature = "media-gstreamer", target_os = "linux")) {
        ElementHandle::find_by_accessible_label(&ui, "Record a voice message")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert!(matches!(
            rx.try_recv().unwrap(),
            Command::Start { chat: 10, .. }
        ));
    }
    publish(
        &ui,
        &Status {
            phase: 6,
            ..Default::default()
        },
    );
    assert_eq!(
        ElementHandle::find_by_element_type_name(&ui, "VoiceRecorder").count(),
        0
    );
    publish(
        &ui,
        &Status {
            error: crate::voice_recorder::Error::NoInput.label().into(),
            failure: Some(crate::voice_recorder::Error::NoInput),
            ..Default::default()
        },
    );
    assert_eq!(ui.get_voice_phase(), 0);
    assert!(ui.get_microphone_dialog());
    assert!(ui.get_notice().is_empty());
    let model = ui.get_timeline_rows();
    let key = |text: slint::SharedString| {
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.clone() });
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
        slint::platform::update_timers_and_animations();
    };
    for (w, h) in [(1200., 800.), (800., 600.), (520., 480.), (620., 480.)] {
        ui.window().set_size(slint::LogicalSize::new(w, h));
        let panel = ElementHandle::find_by_element_id(&ui, "MicrophoneDialog::panel")
            .next()
            .unwrap();
        let p = panel.absolute_position();
        let size = panel.size();
        assert!(p.x >= 0. && p.y >= 0. && p.x + size.width <= w && p.y + size.height <= h);
        let ok = ElementHandle::find_by_accessible_label(&ui, "OK, got it")
            .next()
            .unwrap();
        let b = ok.absolute_position();
        let s = ok.size();
        assert!(
            b.x >= p.x
                && b.y >= p.y
                && b.x + s.width <= p.x + size.width + 0.5
                && b.y + s.height <= p.y + size.height + 0.5
        );
    }
    ui.invoke_focus_composer(); // An async completion must not focus behind the modal.
    key("x".into());
    assert!(ui.get_draft().is_empty());
    assert!(ui.get_timeline_rows() == model);
    key(slint::platform::Key::Escape.into());
    assert!(!ui.get_microphone_dialog());
    if cfg!(all(feature = "media-gstreamer", target_os = "linux")) {
        ElementHandle::find_by_accessible_label(&ui, "Record a voice message")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert!(matches!(
            rx.try_recv().unwrap(),
            Command::Start { chat: 10, .. }
        ));
    }
    publish(
        &ui,
        &Status {
            failure: Some(crate::voice_recorder::Error::NoInput),
            error: crate::voice_recorder::Error::NoInput.label().into(),
            ..Default::default()
        },
    );
    key(slint::platform::Key::Return.into());
    assert!(!ui.get_microphone_dialog());
    assert!(
        rx.try_recv().is_err(),
        "dismissing the dialog must not start/send a recording"
    );
    publish(
        &ui,
        &Status {
            failure: Some(crate::voice_recorder::Error::NoInput),
            error: crate::voice_recorder::Error::NoInput.label().into(),
            ..Default::default()
        },
    );
    ElementHandle::find_by_accessible_label(&ui, "OK, got it")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(!ui.get_microphone_dialog());
    publish(&ui, &Status::default());
    for (w, h) in [(1200., 800.), (800., 600.), (520., 480.), (620., 480.)] {
        ui.window().set_size(slint::LogicalSize::new(w, h));
        for phase in [1, 2, 3, 4, 5] {
            publish(
                &ui,
                &Status {
                    phase,
                    seconds: 65.,
                    levels: vec![35; 64],
                    ..Default::default()
                },
            );
            ui.invoke_layout_timeline();
            assert_eq!(
                ElementHandle::find_by_element_type_name(&ui, "VoiceRecorder").count(),
                1
            );
            for bar in ElementHandle::find_by_element_type_name(&ui, "VoiceRecorder") {
                let p = bar.absolute_position();
                let size = bar.size();
                assert!(size.height <= 44.1);
                for child in bar
                    .query_descendants()
                    .match_type_name("IconButton")
                    .find_all()
                {
                    let c = child.absolute_position();
                    let s = child.size();
                    assert!(c.x >= p.x - 0.5 && c.x + s.width <= p.x + size.width + 0.5);
                    assert!(c.y >= p.y - 0.5 && c.y + s.height <= p.y + size.height + 0.5);
                }
            }
        }
    }
    ui.set_draft("Keep this draft".into());
    publish(
        &ui,
        &Status {
            phase: 1,
            ..Default::default()
        },
    );
    ui.invoke_layout_timeline();
    let model = ui.get_timeline_rows();
    let height = ui.get_timeline_height();
    let y = ui.get_timeline_y();
    let count = model.row_count();
    for tick in 0..1000 {
        publish(
            &ui,
            &Status {
                phase: 1,
                seconds: tick as f64 / 10.,
                levels: vec![(tick % 100) as u8; 64],
                ..Default::default()
            },
        );
    }
    assert!(ui.get_timeline_rows() == model);
    assert_eq!(ui.get_timeline_height(), height);
    assert_eq!(ui.get_timeline_y(), y);
    assert_eq!(ui.get_timeline_rows().row_count(), count);
    assert_eq!(ui.get_draft().as_str(), "Keep this draft");
    while rx.try_recv().is_ok() {}
    ui.set_selected_id(2);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    assert!(matches!(rx.try_recv().unwrap(), Command::Cancel));
    publish(&ui, &Status::default());
    ui.hide().unwrap();
}
