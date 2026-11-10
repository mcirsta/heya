//! Status-button callbacks and a small snapshot, independent of chat models.
use crate::{
    AppWindow,
    own_account::{self, Action, Command, Snapshot, Visibility},
};
use slint::ComponentHandle;
use tokio::sync::{mpsc, watch};

fn submit(ui: &AppWindow, commands: &mpsc::Sender<Command>, action: Action) -> Option<i32> {
    if ui.get_own_pending() || (ui.get_own_busy() && !matches!(action, Action::LoadAbout)) {
        return None;
    }
    let request = ui.get_own_request().wrapping_add(1);
    if commands.try_send(Command { request, action }).is_err() {
        ui.invoke_show_error("Account actions are unavailable. Please try again.".into());
        return None;
    }
    ui.set_own_request(request);
    ui.set_own_pending(true);
    ui.set_own_error("".into());
    Some(request)
}

pub fn attach(
    ui: &AppWindow,
    sources: watch::Receiver<own_account::Sources>,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) -> impl Future<Output = ()> + Send + use<> {
    let (commands, requests) = mpsc::channel(4);
    let (updates, snapshots) = watch::channel(Snapshot::default());
    bind(ui, commands);
    let weak = ui.as_weak();
    async move {
        tokio::join!(
            own_account::run(requests, sources, preferences, updates),
            crate::live_presentation::present_latest(weak, snapshots, publish)
        );
    }
}

fn bind(ui: &AppWindow, commands: mpsc::Sender<Command>) {
    let weak = ui.as_weak();
    let tx = commands.clone();
    ui.on_choose_own_status(move |mode| {
        let Some(ui) = weak.upgrade() else { return };
        if mode == 2 {
            ui.set_own_pending(false);
            if !ui.get_demo_mode() {
                ui.invoke_disconnect_account();
            }
            return;
        }
        if !(0..=1).contains(&mode) {
            return;
        }
        let invisible = mode == 1;
        if !ui.get_demo_mode() && ui.get_connection_state() != 5 {
            if ui.get_connection_blocked() || ui.get_storage_first_run() {
                ui.invoke_show_error("Open Settings → Account to finish connecting.".into());
            } else if !matches!(ui.get_connection_state(), 1..=7) {
                ui.invoke_preference_changed("appear_offline".into(), i32::from(invisible));
                ui.invoke_connect_account();
            }
        } else {
            submit(
                &ui,
                &tx,
                Action::Presence(Visibility::from_preference(invisible)),
            );
        }
    });
    let weak = ui.as_weak();
    let tx = commands.clone();
    ui.on_edit_status_message(move || {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_own_pending() {
            ui.invoke_show_error(
                "Wait for the current account action to finish, then try again.".into(),
            );
            return;
        }
        ui.set_own_editor_open(true);
        ui.set_own_editor_request(0);
        ui.set_own_about_dirty(false);
        ui.set_own_about_draft(ui.get_own_about());
        ui.set_own_error("".into());
        if let Some(request) = submit(&ui, &tx, Action::LoadAbout) {
            ui.set_own_editor_request(request);
        }
    });
    let weak = ui.as_weak();
    ui.on_save_status_message(move || {
        let Some(ui) = weak.upgrade() else { return };
        if let Some(request) = submit(
            &ui,
            &commands,
            Action::SaveAbout(ui.get_own_about_draft().into()),
        ) {
            ui.set_own_editor_request(request);
        }
    });
}

fn publish(ui: &AppWindow, state: &Snapshot) {
    ui.set_own_busy(state.busy);
    ui.set_own_visibility(if !state.connected {
        -1
    } else {
        match state.visibility {
            None => -1,
            Some(Visibility::Online) => 0,
            Some(Visibility::Invisible) => 1,
        }
    });
    ui.set_own_warning(state.warning.as_str().into());
    ui.set_own_about_known(state.about.is_some());
    ui.set_own_about(state.about.as_deref().unwrap_or_default().into());
    if ui.get_own_editor_open() && !ui.get_own_about_dirty() {
        ui.set_own_about_draft(ui.get_own_about());
    }
    if let Some(reply) = &state.reply
        && ui.get_own_pending()
        && reply.request == ui.get_own_request()
    {
        ui.set_own_pending(false);
        ui.set_own_error(reply.error.as_str().into());
        if !reply.error.is_empty() && !ui.get_own_editor_open() {
            ui.invoke_show_error(reply.error.as_str().into());
        } else if reply.error.is_empty()
            && reply.kind == 2
            && reply.request == ui.get_own_editor_request()
            && ui.get_own_editor_open()
            && Some(ui.get_own_about_draft().as_str()) == state.about.as_deref()
        {
            ui.invoke_close_status_editor();
        }
    }
    if !state.busy
        && let Some(visibility) = state.visibility
    {
        let invisible = visibility == Visibility::Invisible;
        if invisible != ui.get_appear_offline() {
            ui.invoke_preference_changed("appear_offline".into(), i32::from(invisible));
        }
    }
}

#[cfg(test)]
#[path = "presentation_own_account_tests.rs"]
pub mod tests;

pub fn demo(
    ui: &AppWindow,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) -> impl Future<Output = ()> + Send + use<> {
    struct Demo(std::sync::Mutex<String>);
    #[async_trait::async_trait]
    impl own_account::Source for Demo {
        async fn presence(&self, _: Visibility) -> Result<(), &'static str> {
            Ok(())
        }
        async fn about(&self) -> Result<String, &'static str> {
            Ok(self.0.lock().unwrap().clone())
        }
        async fn save_about(&self, text: &str) -> Result<(), &'static str> {
            *self.0.lock().unwrap() = text.into();
            Ok(())
        }
    }
    let (sources, receiver) = watch::channel(Some(std::sync::Arc::new(Demo(std::sync::Mutex::new(
        "Hello from Heya".into(),
    )))
        as std::sync::Arc<dyn own_account::Source>));
    let task = attach(ui, receiver, preferences);
    async move {
        let _keep_source = sources;
        task.await;
    }
}
