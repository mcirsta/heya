//! Rejected controls must leave current playback and cancellation state alone.
use crate::{AppWindow, fixtures, presentation, ui::AudioCall};
use slint::{ComponentHandle, Model};
use std::{cell::Cell, rc::Rc};
use tokio::sync::{mpsc, watch};

fn conversation(chat: i32) -> (AppWindow, i32) {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_connection_state(5);
    let mut demo = fixtures::demo();
    demo.select(chat);
    let view = demo.conversation();
    presentation::conversation(&ui, &view, true);
    let message = view.messages[0].id;
    (ui, message)
}

pub(crate) fn check_ui() {
    {
        let (ui, id) = conversation(10);
        let stops = Rc::new(Cell::new(0));
        let count = stops.clone();
        ui.on_video_control(move |_, _, _| count.set(count.get() + 1));
        let (tx, mut rx) = mpsc::channel(1);
        assert!(tx.try_send(crate::audio::Command::Stop).is_ok());
        crate::presentation_audio::bind(&ui, tx);
        ui.invoke_audio_control(id, 0, 0.);
        assert_eq!(
            stops.get(),
            0,
            "a rejected audio command must not stop video"
        );
        assert!(!ui.get_notice().is_empty());
        rx.try_recv().unwrap();
        ui.invoke_audio_control(-999, 0, 0.);
        assert_eq!(
            stops.get(),
            0,
            "an invalid audio target must not stop video"
        );
        ui.invoke_audio_control(id, 0, 0.);
        assert!(matches!(rx.try_recv().unwrap(), crate::audio::Command::Toggle(i) if i==id));
        assert_eq!(stops.get(), 1);
    }
    {
        let (ui, _) = conversation(14);
        let id = ui
            .get_timeline_rows()
            .iter()
            .find(|r| r.message.has_video)
            .unwrap()
            .message
            .id;
        let stops = Rc::new(Cell::new(0));
        let count = stops.clone();
        ui.on_audio_control(move |_, _, _| count.set(count.get() + 1));
        let (tx, mut rx) = mpsc::channel(1);
        assert!(tx.try_send(crate::video::Command::Stop).is_ok());
        crate::presentation_video::bind(&ui, tx);
        ui.invoke_video_control(id, 0, 0.);
        assert_eq!(
            stops.get(),
            0,
            "a rejected video command must not stop audio"
        );
        assert!(!ui.get_notice().is_empty());
        rx.try_recv().unwrap();
        ui.invoke_video_control(id, 0, 0.);
        assert!(matches!(rx.try_recv().unwrap(), crate::video::Command::Toggle(i) if i==id));
        assert_eq!(stops.get(), 1);
    }
    {
        let (ui, _) = conversation(10);
        let stops = Rc::new(Cell::new(0));
        let count = stops.clone();
        ui.on_audio_control(move |_, _, _| count.set(count.get() + 1));
        let (tx, mut rx) = mpsc::channel(1);
        assert!(tx.try_send(crate::voice::Command::Cancel).is_ok());
        crate::presentation_voice::bind(&ui, tx);
        ui.invoke_voice_control(0);
        assert_eq!(stops.get(), 0, "a rejected recording must not stop audio");
        assert_eq!(ui.get_voice_phase(), 0);
        assert!(!ui.get_notice().is_empty());
        rx.try_recv().unwrap();
        ui.invoke_voice_control(0);
        assert!(matches!(
            rx.try_recv().unwrap(),
            crate::voice::Command::Start { .. }
        ));
        assert_eq!(ui.get_voice_phase(), 6);
        assert_eq!(stops.get(), 1);
    }
    {
        let (ui, _) = conversation(10);
        let stops = Rc::new(Cell::new(0));
        let count = stops.clone();
        ui.on_audio_control(move |_, _, _| count.set(count.get() + 1));
        let (tx, mut rx) = mpsc::channel(1);
        assert!(tx.try_send(crate::calling::Command::Mute).is_ok());
        let (stop, stopped) = watch::channel(true);
        crate::presentation_calling::bind(&ui, tx, stop);
        ui.global::<AudioCall>().invoke_action(0);
        assert_eq!(stops.get(), 0, "a rejected call must not stop audio");
        assert!(
            *stopped.borrow(),
            "a rejected call must preserve cancellation"
        );
        assert_eq!(ui.global::<AudioCall>().get_phase(), 0);
        assert!(ui.get_microphone_dialog());
        rx.try_recv().unwrap();
        ui.invoke_close_microphone_dialog();
        ui.global::<AudioCall>().invoke_action(0);
        assert!(matches!(
            rx.try_recv().unwrap(),
            crate::calling::Command::Start(_)
        ));
        assert!(!*stopped.borrow());
        assert_eq!(stops.get(), 1);
        assert_eq!(ui.global::<AudioCall>().get_phase(), 2);
    }
    {
        let (ui, _) = conversation(10);
        let (tx, _rx) = mpsc::channel(1);
        assert!(tx.try_send(crate::downloads::Command::Cancel).is_ok());
        crate::presentation_downloads::bind(&ui, tx);
        for action in [0, 1, 2] {
            ui.set_notice("".into());
            ui.invoke_download_control(action);
            assert!(
                !ui.get_notice().is_empty(),
                "a rejected transfer control needs feedback"
            );
        }
    }
}
