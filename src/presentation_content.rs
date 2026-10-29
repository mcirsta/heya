//! Small UI handoff for embedded thumbnails and explicitly clicked actions.
use crate::{
    AppWindow,
    message_content::{self, Target},
    presentation::timeline,
};
use slint::{ComponentHandle, Model};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, watch},
};

#[cfg(test)]
#[path = "presentation_content_tests.rs"]
mod tests;
#[cfg(test)]
pub(crate) use tests::check_ui;

pub fn bind_images(ui: &AppWindow) -> watch::Receiver<Vec<i32>> {
    let (demand, receiver) = watch::channel(Vec::new());
    let weak = ui.as_weak();
    ui.on_request_message_images(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let ids = ui
            .get_timeline_rows()
            .iter()
            .filter(|r| r.message.has_thumbnail)
            .map(|r| r.message.id)
            .take(crate::message_thumbnails::MAX_DEMAND)
            .collect();
        demand.send_if_modified(|old| {
            if *old == ids {
                false
            } else {
                *old = ids;
                true
            }
        });
    });
    receiver
}

pub fn publish_images(ui: &AppWindow, snapshot: &crate::message_thumbnails::Snapshot) {
    timeline::images(ui, snapshot);
}

pub fn bind_actions(ui: &AppWindow, runtime: &Runtime) {
    let (requests, mut pending) = mpsc::channel::<String>(8);
    let weak = ui.as_weak();
    ui.on_activate_message(move |id, index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let target = if index == -1 {
            crate::presentation_contact_info::item(&ui, id)
                .filter(|row| row.kind == crate::contact_info::Kind::Link as i32)
                .map(|row| Target::Url(row.url.to_string()))
                .unwrap_or_default()
        } else {
            timeline::action(&ui, id, index)
        };
        match target {
            Target::Copy(value) => ui.invoke_copy_value(value.into()),
            Target::Url(value) => {
                if let Some(url) = message_content::web_url(&value) {
                    if requests.try_send(url).is_err() {
                        ui.set_notice("Opening links is busy. Please try again.".into());
                    }
                } else {
                    ui.set_notice("This link cannot be opened.".into());
                }
            }
            Target::Unavailable => ui.set_notice("This action is available on your phone.".into()),
        }
    });
    let weak = ui.as_weak();
    runtime.spawn(async move {
        while let Some(url) = pending.recv().await {
            // One desktop launcher at a time. Never pass a URL to a shell or log it.
            let result = tokio::task::spawn_blocking(move || webbrowser::open(&url)).await;
            if !matches!(result, Ok(Ok(()))) {
                let _ = weak.upgrade_in_event_loop(|ui| {
                    ui.set_notice(
                        "Could not open the browser. You can copy the link instead.".into(),
                    )
                });
            }
        }
    });
}
