//! Constant-size call presentation. PCM never enters a UI callback.
use crate::{
    AppWindow,
    archive::Archive,
    calling::{self, Command, Status},
    preferences::Preferences,
    ui::AudioCall,
};
use slint::ComponentHandle;
use tokio::sync::{mpsc, watch};
pub fn attach(
    ui: &AppWindow,
    archives: watch::Receiver<Option<Archive>>,
    sources: watch::Receiver<calling::Sources>,
    preferences: watch::Receiver<Preferences>,
) -> impl std::future::Future<Output = ()> + Send + use<> {
    let (commands, requests) = mpsc::channel(8);
    let (stop, stopped) = watch::channel(false);
    bind(ui, commands, stop);
    let (updates, snapshots) = watch::channel(Status::default());
    let weak = ui.as_weak();
    async move {
        tokio::join!(
            calling::run(
                archives,
                sources,
                requests,
                updates,
                preferences,
                crate::voice_recorder::Input::Microphone,
                stopped
            ),
            crate::live_presentation::present_latest(weak, snapshots, publish)
        );
    }
}
fn show_error(ui: &AppWindow, message: &str) {
    ui.set_audio_error(message.into());
    ui.set_microphone_dialog(true);
}
pub(crate) fn bind(ui: &AppWindow, commands: mpsc::Sender<Command>, stop: watch::Sender<bool>) {
    let weak = ui.as_weak();
    ui.global::<AudioCall>().on_action(move |action| {
        let Some(ui) = weak.upgrade() else { return };
        let command = match action {
            0 => {
                if ui.global::<AudioCall>().get_phase() != 0 {
                    return;
                }
                if ui.get_connection_state() != 5 {
                    show_error(&ui, calling::Error::Offline.label());
                    return;
                }
                if ui.get_voice_phase() != 0 {
                    show_error(
                        &ui,
                        "Finish or discard your voice message before making a call.",
                    );
                    return;
                }
                Command::Start(ui.get_selected_id())
            }
            1 => {
                if !matches!(ui.get_voice_phase(), 0 | 4)
                    || ui.global::<AudioCall>().get_phase() != 1
                {
                    return;
                }
                Command::Answer(ui.global::<AudioCall>().get_offer().into())
            }
            2 => Command::Decline(ui.global::<AudioCall>().get_offer().into()),
            // Hangup bypasses queued controls, including a burst of mute clicks.
            3 => {
                stop.send_replace(true);
                return;
            }
            4 => {
                if !ui.global::<AudioCall>().get_can_mute() {
                    return;
                }
                Command::Mute
            }
            5 => {
                if ui.global::<AudioCall>().get_phase() != 6 {
                    return;
                }
                Command::Dismiss(ui.global::<AudioCall>().get_offer().into())
            }
            _ => return,
        };
        match commands.try_reserve() {
            Ok(permit) => {
                if action == 0 || action == 1 {
                    if action == 1 && ui.get_voice_playing() {
                        ui.invoke_voice_control(4);
                    }
                    ui.invoke_audio_control(0, 3, 0.);
                    ui.invoke_video_control(0, 3, 0.);
                    stop.send_replace(false);
                    let global = ui.global::<AudioCall>();
                    global.set_error("".into());
                    global.set_phase(2);
                }
                permit.send(command);
            }
            Err(_) => {
                ui.global::<AudioCall>()
                    .set_error("Call controls are busy. Please try again.".into());
                if !ui.global::<AudioCall>().get_shown() {
                    show_error(&ui, "Call controls are busy. Please try again.");
                }
            }
        }
    });
}
pub fn publish(ui: &AppWindow, status: &Status) {
    let global = ui.global::<AudioCall>();
    let before = global.get_phase();
    let before_chat = global.get_chat();
    let error = status.error.map_or("", |e| e.label());
    let report_error =
        status.error.is_some() && (before != status.phase || global.get_error() != error);
    global.set_chat(status.chat);
    global.set_offer(status.offer.as_str().into());
    global.set_available(status.available);
    global.set_phase(status.phase);
    global.set_title(status.title.as_str().into());
    let initials: String = status
        .title
        .split_whitespace()
        .filter_map(|s| s.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    global.set_initials(initials.into());
    let seconds = status.seconds;
    let time = if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    };
    global.set_time(time.into());
    global.set_muted(status.muted);
    global.set_can_mute(status.can_mute);
    global.set_error(error.into());
    if before == 2 && matches!(status.phase, 3..=5) {
        ui.invoke_audio_control(0, 3, 0.);
        ui.invoke_video_control(0, 3, 0.);
    }
    if report_error {
        if status.error == Some(calling::Error::Audio(crate::voice_recorder::Error::NoInput)) {
            show_error(ui, "");
        } else if status.phase == 0 {
            show_error(ui, error);
        }
    }
    if before > 0 && status.phase == 0 {
        ui.invoke_focus_composer();
    }
    if before_chat != status.chat {
        ui.invoke_request_avatars();
    } else {
        crate::presentation_avatars::refresh(ui);
    }
}
#[cfg(test)]
pub(crate) fn check_ui() {
    use i_slint_backend_testing::ElementHandle;
    let ui = AppWindow::new().unwrap();
    let sent = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = sent.clone();
    ui.on_send_message(move |_, _| count.set(count.get() + 1));
    ui.set_show_conversation(true);
    ui.show().unwrap();
    ui.set_draft("Synthetic draft must stay here".into());
    ui.set_connection_state(5);
    ui.set_selected_id(77);
    let (commands, mut requests) = mpsc::channel(8);
    let (stop, stopped) = watch::channel(false);
    bind(&ui, commands, stop);
    for width in [520., 800., 1200.] {
        ui.window().set_size(slint::LogicalSize::new(width, 480.));
        for phase in [1, 2, 3, 4, 5, 6] {
            publish(
                &ui,
                &Status {
                    available: true,
                    phase,
                    can_mute: matches!(phase, 3..=5),
                    offer: if phase == 2 {
                        "synthetic-offer".into()
                    } else {
                        String::new()
                    },
                    title: "Synthetic caller with a long Unicode name · 你好".into(),
                    seconds: 125,
                    ..Default::default()
                },
            );
            let panel = ElementHandle::find_by_element_id(&ui, "CallPanel::panel")
                .next()
                .unwrap();
            let p = panel.absolute_position();
            let s = panel.size();
            assert!(
                p.x >= 0. && p.y >= 0. && p.x + s.width <= width + 0.5 && p.y + s.height <= 480.5
            );
            for text in panel.query_descendants().match_type_name("Text").find_all() {
                let t = text.absolute_position();
                let size = text.size();
                assert!(
                    t.x >= p.x - 0.5
                        && t.y >= p.y - 0.5
                        && t.x + size.width <= p.x + s.width + 0.5
                        && t.y + size.height <= p.y + s.height + 0.5
                );
            }
            let circles: Vec<_> =
                ElementHandle::find_by_element_id(&ui, "CallAction::circle").collect();
            let captions: Vec<_> =
                ElementHandle::find_by_element_id(&ui, "CallAction::caption").collect();
            assert_eq!(circles.len(), if phase == 6 { 0 } else { 2 });
            assert_eq!(circles.len(), captions.len());
            for (circle, caption) in circles.into_iter().zip(captions) {
                assert!(
                    circle.absolute_position().y + circle.size().height + 4.
                        <= caption.absolute_position().y,
                    "call labels must sit below the round buttons"
                );
            }
        }
    }
    ui.global::<AudioCall>().set_phase(0);
    ui.set_connection_state(2);
    let button = ElementHandle::find_by_accessible_label(&ui, "Start a voice call")
        .next()
        .unwrap();
    assert_eq!(button.accessible_enabled(), Some(true));
    workflow_tests::click_button(&ui, "Start a voice call");
    assert!(
        ui.get_microphone_dialog(),
        "a click while the connection changes must report the error instead of silently returning"
    );
    assert!(requests.try_recv().is_err());
    ui.invoke_close_microphone_dialog();
    ui.set_connection_state(5);
    ui.set_voice_phase(1);
    workflow_tests::click_button(&ui, "Start a voice call");
    assert!(ui.get_microphone_dialog());
    assert!(ui.get_audio_error().contains("voice message"));
    assert!(requests.try_recv().is_err());
    ui.invoke_close_microphone_dialog();
    ui.set_voice_phase(0);
    ui.global::<AudioCall>().invoke_action(0);
    assert!(matches!(requests.try_recv().unwrap(), Command::Start(77)));
    ui.global::<AudioCall>().invoke_action(0);
    assert!(
        requests.try_recv().is_err(),
        "a second click cannot enqueue or cancel another call"
    );
    assert!(!ui.global::<AudioCall>().get_shown());
    assert_eq!(
        ElementHandle::find_by_accessible_label(&ui, "Message draft")
            .next()
            .unwrap()
            .accessible_enabled(),
        Some(true),
        "preparation does not block typing"
    );
    assert_eq!(
        ElementHandle::find_by_element_type_name(&ui, "CallPanel").count(),
        0
    );
    let button = ElementHandle::find_by_accessible_label(&ui, "Start a voice call")
        .next()
        .unwrap();
    assert_eq!(
        button.accessible_enabled(),
        Some(false),
        "a double click must not cancel the check or start another call"
    );
    publish(
        &ui,
        &Status {
            error: Some(calling::Error::Audio(crate::voice_recorder::Error::NoInput)),
            ..Default::default()
        },
    );
    assert!(ui.get_microphone_dialog());
    assert_eq!(
        ElementHandle::find_by_element_type_name(&ui, "CallPanel").count(),
        0
    );
    ui.invoke_close_microphone_dialog();
    publish(
        &ui,
        &Status {
            available: true,
            error: Some(calling::Error::Audio(crate::voice_recorder::Error::NoInput)),
            ..Default::default()
        },
    );
    assert!(
        !ui.get_microphone_dialog(),
        "availability refresh cannot reopen a dismissed warning"
    );
    assert_eq!(
        ElementHandle::find_by_element_type_name(&ui, "CallPanel").count(),
        0
    );
    for error in [
        calling::Error::Audio(crate::voice_recorder::Error::Permission),
        calling::Error::Audio(crate::voice_recorder::Error::Device),
        calling::Error::Audio(crate::voice_recorder::Error::Disabled),
        calling::Error::Unavailable,
        calling::Error::Offline,
        calling::Error::Network,
    ] {
        publish(
            &ui,
            &Status {
                phase: 2,
                ..Default::default()
            },
        );
        publish(
            &ui,
            &Status {
                error: Some(error),
                ..Default::default()
            },
        );
        assert_eq!(ui.get_audio_error(), error.label());
        assert!(ui.get_microphone_dialog());
        assert_eq!(
            ElementHandle::find_by_element_type_name(&ui, "CallPanel").count(),
            0
        );
        ui.invoke_close_microphone_dialog();
    }
    ui.global::<AudioCall>().set_offer("synthetic-offer".into());
    for (action, expected) in [(1, 1), (2, 2), (4, 4), (5, 5)] {
        if action == 1 {
            ui.global::<AudioCall>().set_phase(1);
        }
        if action == 2 {
            ui.global::<AudioCall>().set_phase(1);
        }
        if action == 4 {
            ui.global::<AudioCall>().set_phase(5);
            ui.global::<AudioCall>().set_can_mute(true);
        }
        if action == 5 {
            ui.global::<AudioCall>().set_phase(6);
        }
        ui.global::<AudioCall>().invoke_action(action);
        let got = match requests.try_recv().unwrap() {
            Command::Answer(id) => {
                assert_eq!(id, "synthetic-offer");
                1
            }
            Command::Decline(id) => {
                assert_eq!(id, "synthetic-offer");
                2
            }
            Command::Mute => 4,
            Command::Dismiss(_) => 5,
            _ => 0,
        };
        assert_eq!(got, expected);
    }
    ui.global::<AudioCall>().set_phase(5);
    let mute = ElementHandle::find_by_accessible_label(&ui, "Mute")
        .next()
        .unwrap();
    for _ in 0..8 {
        mute.invoke_accessible_default_action();
    }
    ElementHandle::find_by_accessible_label(&ui, "End call")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(
        *stopped.borrow(),
        "hangup is independent of queued commands"
    );
    for _ in 0..8 {
        assert!(matches!(requests.try_recv().unwrap(), Command::Mute));
    }
    assert!(requests.try_recv().is_err());
    ui.invoke_focus_composer();
    for pressed in [true, false] {
        ui.window().dispatch_event(if pressed {
            slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Return.into(),
            }
        } else {
            slint::platform::WindowEvent::KeyReleased {
                text: slint::platform::Key::Return.into(),
            }
        });
    }
    assert_eq!(
        sent.get(),
        0,
        "Enter in the active call cannot send the hidden draft"
    );
    for _ in 0..12 {
        for key in [slint::platform::Key::Tab, slint::platform::Key::Return] {
            let text: slint::SharedString = key.into();
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.clone() });
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
        }
    }
    assert_eq!(
        sent.get(),
        0,
        "tabbing through a call must not reach a hidden send control"
    );
    publish(
        &ui,
        &Status {
            phase: 6,
            error: Some(calling::Error::Audio(crate::voice_recorder::Error::NoInput)),
            ..Default::default()
        },
    );
    assert!(ui.get_microphone_dialog());
    ui.invoke_close_microphone_dialog();
    slint::platform::update_timers_and_animations();
    for pressed in [true, false] {
        ui.window().dispatch_event(if pressed {
            slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Return.into(),
            }
        } else {
            slint::platform::WindowEvent::KeyReleased {
                text: slint::platform::Key::Return.into(),
            }
        });
    }
    assert_eq!(
        sent.get(),
        0,
        "closing a microphone warning must return focus to the call"
    );
    assert_eq!(ui.get_draft().as_str(), "Synthetic draft must stay here");
    publish(&ui, &Status::default());
    ui.hide().unwrap();
}

#[cfg(test)]
#[path = "presentation_calling_tests.rs"]
pub(crate) mod workflow_tests;
