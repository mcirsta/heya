//! Storage choices use the existing shutdown path before any conversion.
use crate::{
    AppWindow,
    connection::Failure,
    storage::{self, Mode, Status},
};
use slint::ComponentHandle;
use std::{cell::Cell, rc::Rc};

pub struct Restart(pub Rc<Cell<Option<Mode>>>);

pub fn attach(
    ui: &AppWindow,
    runtime: &tokio::runtime::Runtime,
    state: Option<Result<Status, Failure>>,
    changing: bool,
) -> (Restart, tokio::task::JoinHandle<()>) {
    match state {
        Some(Ok(state)) => {
            ui.set_storage_encrypted(state.mode == Mode::Encrypted);
            ui.set_storage_first_run(!state.configured);
            ui.set_storage_dialog(!state.configured || changing);
        }
        Some(Err(error)) => {
            ui.set_storage_error(error.message().into());
            ui.set_storage_dialog(true);
        }
        None => {}
    }
    ui.set_storage_target(ui.get_storage_encrypted());
    ui.set_storage_busy(changing);
    let restart = Rc::new(Cell::new(None));
    let (commands, mut rx) = tokio::sync::mpsc::channel::<Mode>(1);
    let weak = ui.as_weak();
    let task = runtime.spawn(async move {
        while let Some(mode) = rx.recv().await {
            let result = crate::local_worker::run(move || storage::configure(mode))
                .await
                .unwrap_or(Err(Failure::DataDirectory));
            let _ = weak.upgrade_in_event_loop(move |ui| complete(&ui, result.map(|s| s.mode)));
        }
    });
    ui.invoke_refresh_settings();
    bind(ui, commands, restart.clone());
    (Restart(restart), task)
}

fn bind(
    ui: &AppWindow,
    commands: tokio::sync::mpsc::Sender<Mode>,
    restart: Rc<Cell<Option<Mode>>>,
) {
    let weak = ui.as_weak();
    ui.on_open_storage_choice(move |encrypted| {
        if let Some(ui) = weak.upgrade()
            && !ui.get_storage_busy()
        {
            ui.set_storage_target(encrypted);
            ui.set_storage_error("".into());
            ui.set_storage_dialog(true);
        }
    });
    let slot = restart.clone();
    let weak = ui.as_weak();
    ui.on_confirm_storage_choice(move || {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_storage_busy() || !ui.get_storage_error().is_empty() {
            return;
        }
        let mode = if ui.get_storage_target() {
            Mode::Encrypted
        } else {
            Mode::Unencrypted
        };
        if ui.get_demo_mode() {
            ui.set_storage_encrypted(mode == Mode::Encrypted);
            ui.set_storage_dialog(false);
            ui.invoke_refresh_settings();
        } else if ui.get_storage_first_run() {
            if commands.try_send(mode).is_ok() {
                ui.set_storage_busy(true);
            } else {
                ui.set_storage_error(
                    "The storage service is unavailable. Close Heya and try again.".into(),
                );
            }
        } else if ui.global::<crate::ui::AudioCall>().get_phase() != 0
            || ui.get_voice_phase() != 0
            || ui.get_attachment_phase() != 0
        {
            ui.set_storage_error(
                "Finish or cancel your call, voice message or attachment before changing storage."
                    .into(),
            );
        } else if ui.get_storage_target() == ui.get_storage_encrypted() {
            ui.set_storage_dialog(false);
        } else {
            slot.set(Some(mode));
            ui.set_storage_busy(true);
            if slint::quit_event_loop().is_err() {
                slot.set(None);
                ui.set_storage_busy(false);
                ui.set_storage_error("Heya could not restart. Close it and try again.".into());
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_close_storage_choice(move || {
        if let Some(ui) = weak.upgrade()
            && !ui.get_storage_busy()
        {
            if ui.get_storage_first_run() {
                let _ = slint::quit_event_loop();
            } else {
                ui.set_storage_dialog(false);
            }
        }
    });
}

pub fn complete(ui: &AppWindow, result: Result<Mode, Failure>) {
    ui.set_storage_busy(false);
    match result {
        Ok(mode) => {
            ui.set_storage_encrypted(mode == Mode::Encrypted);
            ui.set_storage_first_run(false);
            ui.set_storage_error("".into());
            ui.set_storage_dialog(false);
        }
        Err(error) => {
            ui.set_storage_error(error.message().into());
            ui.set_storage_dialog(true);
        }
    }
    ui.invoke_refresh_settings();
}

#[cfg(test)]
pub(crate) fn check_ui() {
    use i_slint_backend_testing::ElementHandle;
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(false);
    ui.set_storage_first_run(true);
    ui.set_storage_dialog(true);
    ui.set_storage_target(false);
    let slot = Rc::new(Cell::new(None));
    let (commands, mut rx) = tokio::sync::mpsc::channel(1);
    bind(&ui, commands, slot.clone());
    ui.show().unwrap();
    for (width, height) in [(1200., 800.), (520., 480.), (620., 600.)] {
        ui.window().set_size(slint::LogicalSize::new(width, height));
        let dialog = ElementHandle::find_by_element_id(&ui, "StorageDialog::dialog")
            .next()
            .unwrap();
        let p = dialog.absolute_position();
        let s = dialog.size();
        assert!(
            p.x >= 0.
                && p.y >= 0.
                && p.x + s.width <= width + 0.5
                && p.y + s.height <= height + 0.5
        );
        let notice = ElementHandle::find_by_element_id(&ui, "StorageDialog::notice")
            .next()
            .unwrap();
        let message = notice.accessible_label().unwrap();
        assert_eq!(message.matches('.').count(), 1, "one concise explanation");
        assert!(message.split_whitespace().count() <= 22);
        assert!(s.height < 340., "the choice should remain a compact card");
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Continue")
                .next()
                .is_some()
        );
    }
    ui.invoke_confirm_storage_choice();
    assert_eq!(rx.try_recv().unwrap(), Mode::Unencrypted);
    assert!(ui.get_storage_busy());
    ui.invoke_confirm_storage_choice();
    assert!(
        rx.try_recv().is_err(),
        "a delayed choice cannot be submitted twice"
    );
    complete(&ui, Err(Failure::DataDirectory));
    assert!(ui.get_storage_first_run() && ui.get_storage_encrypted());
    assert!(!ui.get_storage_busy());
    assert!(!ui.get_storage_error().is_empty());
    ui.invoke_open_storage_choice(false);
    ui.invoke_confirm_storage_choice();
    assert_eq!(rx.try_recv().unwrap(), Mode::Unencrypted);
    complete(&ui, Ok(Mode::Unencrypted));
    assert!(!ui.get_storage_first_run() && !ui.get_storage_dialog() && !ui.get_storage_encrypted());
    ui.invoke_open_storage_choice(true);
    ui.set_attachment_phase(1);
    ui.invoke_confirm_storage_choice();
    assert!(slot.get().is_none());
    assert!(!ui.get_storage_error().is_empty());
    ui.set_attachment_phase(0);
    ui.invoke_close_storage_choice();
    ui.set_storage_first_run(true);
    drop(rx);
    ui.invoke_open_storage_choice(true);
    ui.invoke_confirm_storage_choice();
    assert!(!ui.get_storage_busy());
    assert!(ui.get_storage_dialog() && !ui.get_storage_error().is_empty());
    ui.set_storage_dialog(false);
    ui.set_storage_first_run(false);
    let (requests, mut receiver) = tokio::sync::watch::channel(crate::connection::Request::Idle);
    crate::live_presentation::bind(&ui, requests);
    crate::live_presentation::publish(
        &ui,
        &crate::connection::Snapshot::new(crate::connection::Phase::WaitingForKeyStore),
    );
    assert_eq!(ui.get_connection_state(), 1);
    assert!(ui.get_connection_waiting_key_store());
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "Waiting for the OS key store…")
            .next()
            .is_some()
    );
    ui.invoke_connect_account();
    assert!(
        !receiver.has_changed().unwrap(),
        "waiting must not enqueue a duplicate connection"
    );
    ui.invoke_disconnect_account();
    assert_eq!(
        *receiver.borrow_and_update(),
        crate::connection::Request::Disconnect
    );
    crate::live_presentation::publish(
        &ui,
        &crate::connection::Snapshot::new(crate::connection::Phase::Stopped),
    );
    assert!(!ui.get_connection_waiting_key_store());
    ui.hide().unwrap();
}
