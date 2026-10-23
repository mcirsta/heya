//! Constant-size playback bindings. Progress never rebuilds the timeline model.
use crate::{
    AppWindow,
    archive::Archive,
    audio::{self, Command, Status},
    ui::AudioPlayback,
};
use slint::{ComponentHandle, Model};
use tokio::sync::{mpsc, watch};

pub fn duration(seconds: f64) -> String {
    let seconds = seconds.clamp(0., 6. * 3600.) as u64;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}
pub fn waveform(levels: &[u8]) -> String {
    // One native path, not a control per sample. Exactly 64 two-pixel bars.
    use std::fmt::Write;
    let mut path = String::with_capacity(1800);
    for i in 0..64 {
        let level = if levels.is_empty() {
            12
        } else {
            levels[i * levels.len() / 64].min(100)
        };
        let height = 2. + f32::from(level) * 0.26;
        let _ = write!(
            path,
            "M {} {:.1} h 2 v {:.1} h -2 Z ",
            i * 3 + 1,
            (30. - height) / 2.,
            height
        );
    }
    path
}
pub fn publish(ui: &AppWindow, status: &Status) {
    let global = ui.global::<AudioPlayback>();
    global.set_id(status.id);
    global.set_state(status.phase);
    global.set_fraction(if status.duration > 0. {
        (status.position / status.duration).clamp(0., 1.) as f32
    } else {
        0.
    });
    global.set_time(duration(status.position).into());
    global.set_speed(format!("{}×", status.speed.max(1.)).into());
    global.set_error(status.error.map_or("", |e| e.label()).into());
    if let Some(error) = status.error {
        ui.set_notice(error.label().into());
    }
}

pub(crate) fn bind(ui: &AppWindow, commands: mpsc::Sender<Command>) {
    let weak = ui.as_weak();
    ui.on_audio_control(move |id, action, value| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if action != 3 && ui.get_voice_phase() > 0 {
            return;
        }
        if action != 3
            && !ui
                .get_timeline_rows()
                .iter()
                .any(|row| row.message.id == id && row.message.has_audio)
        {
            return;
        }
        let command = match action {
            0 => Command::Toggle(id),
            1 => Command::Seek(id, value as f64),
            2 => Command::Speed,
            3 => Command::Stop,
            _ => return,
        };
        match commands.try_reserve() {
            Ok(permit) => {
                if action == 0 {
                    ui.invoke_video_control(0, 3, 0.);
                }
                permit.send(command);
            }
            Err(_) => ui.set_notice("Audio is busy. Please try again.".into()),
        }
    });
}
pub fn attach(
    ui: &AppWindow,
    archives: watch::Receiver<Option<Archive>>,
    sources: watch::Receiver<audio::Sources>,
    demo: bool,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) -> impl std::future::Future<Output = ()> + Send + use<> {
    let (tx, rx) = mpsc::channel(16);
    bind(ui, tx);
    let (updates, snapshots) = watch::channel(Status::default());
    let weak = ui.as_weak();
    async move {
        tokio::join!(
            audio::run(archives, sources, rx, updates, demo, preferences),
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
    let (commands, mut rx) = mpsc::channel(16);
    bind(&ui, commands);
    let mut demo = crate::fixtures::demo();
    demo.select(10);
    let view = demo.conversation();
    for dark in [false, true] {
        ui.global::<crate::Theme>().set_dark(dark);
        for (width, height) in [(1200., 900.), (520., 600.), (800., 600.), (620., 600.)] {
            ui.window().set_size(slint::LogicalSize::new(width, height));
            crate::presentation::conversation(&ui, &view, true);
            ui.show().unwrap();
            ui.invoke_layout_timeline();
            let model = ui.get_timeline_rows();
            let height = ui.get_timeline_height();
            let offset = ui.get_timeline_y();
            let id = view.messages[0].id;
            for phase in [1, 2, 3, 4, 0] {
                publish(
                    &ui,
                    &Status {
                        id,
                        phase,
                        position: 2.,
                        duration: 4.,
                        speed: 1.5,
                        ..Default::default()
                    },
                );
                assert!(
                    ui.get_timeline_rows() == model,
                    "playback must not replace the model"
                );
                assert_eq!(ui.get_timeline_height(), height);
                assert_eq!(ui.get_timeline_y(), offset);
                for player in ElementHandle::find_by_element_type_name(&ui, "AudioPlayer") {
                    assert!(player.size().height <= 48.1);
                    let p = player.absolute_position();
                    let size = player.size();
                    for child in [
                        "AudioPlayer::play",
                        "AudioPlayer::wave",
                        "AudioPlayer::speed",
                    ] {
                        let child = player
                            .query_descendants()
                            .match_id(child)
                            .find_first()
                            .unwrap();
                        let c = child.absolute_position();
                        let cs = child.size();
                        assert!(c.x >= p.x - 0.5 && c.x + cs.width <= p.x + size.width + 0.5);
                        assert!(c.y >= p.y - 0.5 && c.y + cs.height <= p.y + size.height + 0.5);
                    }
                    if let Some(clip) = player
                        .query_descendants()
                        .match_id("AudioPlayer::played-clip")
                        .find_first()
                    {
                        let wave = player
                            .query_descendants()
                            .match_id("AudioPlayer::wave")
                            .find_first()
                            .unwrap();
                        let played = player
                            .query_descendants()
                            .match_id("AudioPlayer::played-wave")
                            .find_first()
                            .unwrap();
                        // Progress reveals the same waveform from its left edge;
                        // implicit centering must not shift or rescale the fill.
                        assert!(
                            (clip.absolute_position().x - wave.absolute_position().x).abs() < 0.5
                        );
                        assert!(
                            (played.absolute_position().x - wave.absolute_position().x).abs() < 0.5
                        );
                        assert!((clip.size().width - wave.size().width * 0.5).abs() < 0.5);
                        assert!((played.size().width - wave.size().width).abs() < 0.5);
                    }
                }
            }
            for bubble in ElementHandle::find_by_element_type_name(&ui, "MessageBubble") {
                if let Some(duration) = bubble
                    .query_descendants()
                    .match_id("AudioPlayer::duration-label")
                    .find_first()
                {
                    let stamp = bubble
                        .query_descendants()
                        .match_id("MessageBubble::stamp")
                        .find_first()
                        .unwrap();
                    assert!(
                        (duration.absolute_position().y - stamp.absolute_position().y).abs() < 0.5,
                        "duration and timestamp must share their line"
                    );
                    assert!(
                        duration.absolute_position().x + duration.size().width
                            <= stamp.absolute_position().x - 7.
                    );
                }
            }
            while rx.try_recv().is_ok() {}
            ui.invoke_audio_control(id, 0, 0.);
            assert!(matches!(rx.try_recv(),Ok(Command::Toggle(i)) if i==id));
            ui.invoke_audio_control(-999, 0, 0.);
            assert!(rx.try_recv().is_err());
        }
    }
    // Long delivery statuses get their own footer instead of covering duration
    // or waveform controls. Playback progress cannot change this geometry.
    let mut long = view.clone();
    for message in std::sync::Arc::make_mut(&mut long.messages) {
        message.time = "10:24 · Unconfirmed · check phone before trying again".into();
    }
    ui.window().set_size(slint::LogicalSize::new(620., 600.));
    crate::presentation::conversation(&ui, &long, true);
    ui.invoke_layout_timeline();
    for bubble in ElementHandle::find_by_element_type_name(&ui, "MessageBubble") {
        if let Some(duration) = bubble
            .query_descendants()
            .match_id("AudioPlayer::duration-label")
            .find_first()
        {
            let stamp = bubble
                .query_descendants()
                .match_id("MessageBubble::stamp")
                .find_first()
                .unwrap();
            assert!(
                duration.absolute_position().y + duration.size().height
                    < stamp.absolute_position().y
            );
        }
    }
    demo.select(6);
    let mut view = demo.conversation();
    let mut messages = demo.chats[5].messages.clone();
    crate::fixtures::fill_audio(&mut messages);
    view.messages = messages.into();
    crate::presentation::conversation(&ui, &view, true);
    ui.invoke_layout_timeline();
    assert!(ui.get_timeline_rows().row_count() < 32);
    assert!(ElementHandle::find_by_element_type_name(&ui, "AudioPlayer").count() < 33);
    ui.hide().unwrap();
}
