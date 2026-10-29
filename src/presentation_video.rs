//! Playback updates one global image/control state; the transcript stays untouched.
use crate::{
    AppWindow,
    ui::VideoPlayback,
    video::{self, Command, Status},
};
use slint::{ComponentHandle, Model};
use tokio::sync::{mpsc, watch};

pub fn publish(ui: &AppWindow, status: &Status) {
    let global = ui.global::<VideoPlayback>();
    if global.get_id() != status.id
        || global.get_frame_number() != status.frame_number
        || (status.frame.is_none() && global.get_frame().size().width > 0)
    {
        global.set_frame(
            status
                .frame
                .as_ref()
                .map(|p| {
                    slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                        &p.rgba, p.width, p.height,
                    ))
                })
                .unwrap_or_default(),
        );
        global.set_frame_number(status.frame_number);
    }
    global.set_id(status.id);
    global.set_state(status.phase);
    global.set_muted(status.muted);
    global.set_fraction(if status.duration > 0. {
        (status.position / status.duration).clamp(0., 1.) as f32
    } else {
        0.
    });
    global.set_time(
        format!(
            "{} / {}",
            crate::presentation_audio::duration(status.position),
            crate::presentation_audio::duration(status.duration)
        )
        .into(),
    );
    global.set_error(status.error.map_or("", |e| e.label()).into());
    if status.phase == 0 || status.phase == 4 {
        ui.set_video_expanded(false);
    }
    if let Some(error) = status.error {
        ui.set_notice(error.label().into());
    }
}

pub(crate) fn bind(ui: &AppWindow, commands: mpsc::Sender<Command>) {
    let weak = ui.as_weak();
    ui.on_video_control(move |id, action, value| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let global = ui.global::<VideoPlayback>();
        let contact_video =
            crate::presentation_contact_info::has(&ui, id, crate::contact_info::Kind::Video);
        let visible = contact_video
            || ui
                .get_timeline_rows()
                .iter()
                .any(|r| r.message.id == id && r.message.has_video && r.message.actionable);
        let command = match action {
            3 => Command::Stop,
            5 => {
                let current = global.get_id();
                if current == 0 {
                    return;
                }
                let hidden = ui.get_settings_open()
                    || ui.get_new_chat_open()
                    || ui.get_account_dialog() != 0
                    || ui.window().is_minimized()
                    || (ui.window().size().width < 620 && !ui.get_show_conversation());
                if !hidden
                    && (ui.get_video_expanded()
                        || ui
                            .get_timeline_rows()
                            .iter()
                            .any(|r| r.message.id == current && r.message.has_video))
                {
                    return;
                }
                Command::Stop
            }
            4 if id == global.get_id() && matches!(global.get_state(), 2 | 3) => {
                ui.set_video_expanded(!ui.get_video_expanded());
                return;
            }
            _ if ui.get_voice_phase() > 0
                || (!visible && !(ui.get_video_expanded() && global.get_id() == id)) =>
            {
                return;
            }
            0 => Command::Toggle(id),
            1 => Command::Seek(id, f64::from(value)),
            2 => Command::Mute(id),
            _ => return,
        };
        match commands.try_reserve() {
            Ok(permit) => {
                if matches!(command, Command::Stop) {
                    ui.set_video_expanded(false);
                } else if action == 0 {
                    ui.invoke_audio_control(0, 3, 0.);
                    if contact_video {
                        ui.set_video_expanded(true);
                    }
                }
                permit.send(command);
            }
            Err(_) => ui.set_notice("Video is busy. Please try again.".into()),
        }
    });
}

pub fn attach(
    ui: &AppWindow,
    archives: watch::Receiver<Option<crate::archive::Archive>>,
    sources: watch::Receiver<video::Sources>,
    demo: bool,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) -> impl std::future::Future<Output = ()> + Send + use<> {
    let (commands, rx) = mpsc::channel(16);
    bind(ui, commands);
    let (updates, snapshots) = watch::channel(Status::default());
    let weak = ui.as_weak();
    async move {
        tokio::join!(
            video::run(archives, sources, rx, updates, demo, preferences),
            crate::live_presentation::present_latest(weak, snapshots, publish)
        );
    }
}

#[cfg(test)]
#[path = "presentation_video_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::check_ui;
