//! Synthetic mouse-click workflows through the call controller and audio helper.
use crate::{
    calling::Error,
    calling_tests::Harness,
    voice_recorder::{Error as AudioError, Input},
};
use std::{sync::atomic::Ordering, time::Duration};

pub(crate) fn click_button(ui: &crate::AppWindow, label: &str) {
    use i_slint_backend_testing::ElementHandle;
    use slint::{
        ComponentHandle,
        platform::{PointerEventButton, WindowEvent},
    };
    let button = ElementHandle::find_by_accessible_label(ui, label)
        .next()
        .unwrap();
    assert_eq!(button.accessible_enabled(), Some(true), "{label}");
    let origin = button.absolute_position();
    let size = button.size();
    let position =
        slint::LogicalPosition::new(origin.x + size.width / 2., origin.y + size.height / 2.);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

/// Click the real header, run the real controller/helper, and consume its UI updates.
/// No account, microphone hardware or signaling transport is attached.
pub(crate) fn check_ui_flow(runtime: &tokio::runtime::Runtime) {
    use i_slint_backend_testing::ElementHandle;
    use slint::ComponentHandle;
    for failure in [
        Some(Error::Audio(AudioError::NoInput)),
        Some(Error::Network),
        Some(Error::Unavailable),
        None,
    ] {
        let input = if failure == Some(Error::Audio(AudioError::NoInput)) {
            Input::Unavailable
        } else {
            Input::Synthetic
        };
        let h = runtime.block_on(Harness::new(input));
        let chat = if failure == Some(Error::Unavailable) {
            runtime
                .block_on(h.archive.create_chat("15550000001-123@g.us".into()))
                .unwrap()
        } else {
            h.chat
        };
        if failure == Some(Error::Network) {
            *h.network.failure.lock().unwrap() = failure;
        }
        h.network.delay.store(failure.is_none(), Ordering::Release);
        let ui = crate::AppWindow::new().unwrap();
        ui.set_demo_mode(true);
        ui.set_selected_id(chat);
        ui.set_chat_is_group(failure == Some(Error::Unavailable));
        ui.set_chat_title("Synthetic caller".into());
        ui.set_show_conversation(true);
        ui.set_connection_state(5);
        ui.window().set_size(slint::LogicalSize::new(1200., 800.));
        crate::presentation_calling::bind(&ui, h.commands.clone(), h.stop.clone());
        let pump = runtime.spawn(crate::live_presentation::present_latest(
            ui.as_weak(),
            h.states.clone(),
            crate::presentation_calling::publish,
        ));
        let step = std::rc::Rc::new(std::cell::Cell::new(0));
        let tick = step.clone();
        let weak = ui.as_weak();
        let network = h.network.clone();
        let began = std::time::Instant::now();
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(10),
            move || {
                let ui = weak.upgrade().unwrap();
                let call = ui.global::<crate::ui::AudioCall>();
                if began.elapsed() > Duration::from_secs(8) {
                    slint::quit_event_loop().unwrap();
                    return;
                }
                match tick.get() {
                    0 if call.get_available() => {
                        click_button(&ui, "Start a voice call");
                        assert!(!call.get_shown());
                        tick.set(1);
                    }
                    1 if ui.get_microphone_dialog() => {
                        let error = failure.expect("a working call must not show an error popup");
                        assert_eq!(call.get_phase(), 0);
                        assert_eq!(
                            ElementHandle::find_by_element_type_name(&ui, "CallPanel").count(),
                            0
                        );
                        assert_eq!(
                            ui.get_audio_error().as_str(),
                            if error == Error::Audio(AudioError::NoInput) {
                                ""
                            } else {
                                error.label()
                            }
                        );
                        click_button(&ui, "OK, got it");
                        assert!(!ui.get_microphone_dialog());
                        tick.set(6);
                        slint::quit_event_loop().unwrap();
                    }
                    1 if failure.is_none()
                        && call.get_phase() == 3
                        && network.opened.load(Ordering::Acquire) > 0 =>
                    {
                        assert!(
                            call.get_shown(),
                            "Calling must be visible while network setup is pending"
                        );
                        assert_eq!(
                            ElementHandle::find_by_accessible_label(&ui, "Mute")
                                .next()
                                .unwrap()
                                .accessible_enabled(),
                            Some(false)
                        );
                        network.resume.notify_one();
                        tick.set(2);
                    }
                    2 if call.get_phase() == 5 => {
                        click_button(&ui, "Mute");
                        tick.set(3);
                    }
                    3 if call.get_muted() => {
                        assert!(call.get_muted());
                        click_button(&ui, "End call");
                        tick.set(4);
                    }
                    4 if call.get_phase() == 6 => {
                        click_button(&ui, "Done");
                        tick.set(5);
                    }
                    5 if call.get_phase() == 0 => {
                        tick.set(6);
                        slint::quit_event_loop().unwrap();
                    }
                    _ => {}
                }
            },
        );
        ui.run().unwrap();
        timer.stop();
        assert_eq!(
            step.get(),
            6,
            "button-to-controller workflow timed out: {failure:?}"
        );
        assert_eq!(
            h.network.opened.load(Ordering::Acquire),
            usize::from(failure.is_none() || failure == Some(Error::Network))
        );
        assert_eq!(
            h.network.ended.load(Ordering::Acquire),
            usize::from(failure.is_none())
        );
        ui.hide().unwrap();
        drop(ui);
        runtime.block_on(h.finish());
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), pump)
                .await
                .unwrap()
                .unwrap();
        });
    }
}
