//! Shared navigation presentation; storage work is queued, never performed here.
use crate::ui::MessageSearchHit;
use crate::{
    AppWindow, live_presentation, message_navigation::Command, messaging::Snapshot,
    presentation::timeline,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::rc::Rc;
use tokio::sync::mpsc;

pub(crate) fn bind_live(ui: &AppWindow, commands: mpsc::Sender<crate::messaging::Command>) {
    bind(ui, move |ui, command| {
        live_presentation::enqueue(
            ui,
            &commands,
            crate::messaging::Command::Navigation(command),
        )
    });
}
pub(crate) fn bind(ui: &AppWindow, submit: impl Fn(&AppWindow, Command) -> bool + 'static) {
    let submit = Rc::new(submit);
    let timer = Rc::new(slint::Timer::default());
    ui.on_search_messages({
        let weak = ui.as_weak();
        let submit = submit.clone();
        move |query| {
            timer.stop();
            let Some(ui) = weak.upgrade() else { return };
            let request = ui.get_message_search_request().wrapping_add(1).max(1);
            ui.set_message_search_request(request);
            ui.set_message_search_hits(ModelRc::default());
            ui.set_message_search_busy(!query.trim().is_empty());
            ui.set_message_search_status(
                if query.trim().is_empty() {
                    "Search this conversation’s saved history"
                } else {
                    "Searching…"
                }
                .into(),
            );
            let command = Command::Search {
                chat: ui.get_selected_id(),
                query: query.into(),
                request,
                revision: ui.get_live_revision(),
            };
            if ui.get_message_search_busy() {
                let weak = weak.clone();
                let submit = submit.clone();
                let mut command = Some(command);
                timer.start(
                    slint::TimerMode::SingleShot,
                    std::time::Duration::from_millis(250),
                    move || {
                        if let Some(ui) = weak.upgrade()
                            && let Some(command) = command.take()
                            && !submit(&ui, command)
                        {
                            ui.set_message_search_busy(false);
                            ui.set_message_search_status(
                                "Search could not start. Please try again.".into(),
                            );
                        }
                    },
                );
            } else {
                submit(&ui, command);
            }
        }
    });
    ui.on_navigate_message({
        let weak = ui.as_weak();
        move |message, quote| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_navigation_request() != ui.get_navigation_shown() {
                return;
            }
            if !ui.get_browsing_history() && message != 0 {
                timeline::save_position(&ui);
            }
            let request = ui.get_navigation_request().wrapping_add(1).max(1);
            if submit(
                &ui,
                Command::Navigate {
                    chat: ui.get_selected_id(),
                    message,
                    quote,
                    request,
                    revision: ui.get_live_revision(),
                },
            ) {
                ui.set_navigation_request(request);
            }
        }
    });
}

pub(crate) fn prepare(ui: &AppWindow, snapshot: &Snapshot) {
    if snapshot.jump.request == ui.get_navigation_request() {
        // Set this before layout can schedule a visible-read callback.
        ui.set_browsing_history(snapshot.jump.browsing);
    }
}
pub(crate) fn publish(ui: &AppWindow, snapshot: &Snapshot) {
    let search = &snapshot.search;
    if ui.get_message_search_open()
        && search.chat == ui.get_selected_id()
        && search.request == ui.get_message_search_request()
        && ui.get_message_search_query() == search.query
    {
        ui.set_message_search_busy(false);
        ui.set_message_search_hits(ModelRc::new(VecModel::from(
            search
                .hits
                .iter()
                .map(|h| MessageSearchHit {
                    id: h.id,
                    text: h.text.clone().into(),
                    time: h.time.clone().into(),
                })
                .collect::<Vec<_>>(),
        )));
        ui.set_message_search_status(if search.query.trim().is_empty() {
            "Search this conversation’s saved history".into()
        } else if search.more {
            "Showing the newest 100 matches · refine your search".into()
        } else if search.hits.is_empty() {
            "No matches in saved history".into()
        } else {
            format!("{} matches in saved history", search.hits.len()).into()
        });
    }
    let jump = snapshot.jump;
    if jump.request == ui.get_navigation_request() && jump.request != ui.get_navigation_shown() {
        if (jump.message != 0 || jump.returning)
            && !timeline::navigate(ui, jump.message, jump.returning)
        {
            ui.set_notice("The original position is no longer available in saved history.".into());
        }
        ui.set_highlighted_message(timeline::display_id(ui, jump.message));
        ui.set_navigation_shown(jump.request);
    }
}

#[cfg(test)]
pub(crate) fn check_ui() {
    use slint::Model;
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(false);
    ui.set_live_chat_view(true);
    ui.set_connection_state(5);
    ui.set_read_window_active(true);
    ui.set_show_conversation(true);
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    ui.show().unwrap();
    let mut state = crate::fixtures::demo();
    state.select(6);
    let view = state.conversation();
    state.load_older(6);
    let mut old_window = state.conversation();
    old_window.messages = old_window.messages[..100].into();
    let old_target = old_window.messages[20].id;
    std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut old_window.messages)[20].content)
        .quote = Some(crate::message_content::Quote {
        sender: "Synthetic quote".into(),
        text: "Original saved text".into(),
    });
    crate::presentation::conversation(&ui, &view, true);
    ui.set_timeline_y(-220.);
    ui.invoke_layout_timeline();
    let old_position = timeline::position(&ui);
    let old_draft = "Draft stays here 👩🏽‍💻";
    ui.set_draft(old_draft.into());
    let (tx, mut rx) = mpsc::channel(8);
    bind_live(&ui, tx);
    ui.invoke_navigate_message(old_target, false);
    let command = rx.try_recv().unwrap();
    let crate::messaging::Command::Navigation(Command::Navigate {
        message, request, ..
    }) = command
    else {
        panic!("navigation expected")
    };
    let snapshot = Snapshot {
        jump: crate::message_navigation::Jump {
            request,
            message,
            browsing: true,
            returning: false,
        },
        ..Snapshot::empty()
    };
    prepare(&ui, &snapshot);
    crate::presentation::conversation(&ui, &old_window, false);
    publish(&ui, &snapshot);
    assert!(ui.get_browsing_history());
    assert_eq!(ui.get_highlighted_message(), message);
    assert_eq!(ui.get_draft(), old_draft);
    assert_eq!(ui.get_read_visible_message(), 0);
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&ui, "Go to quoted message")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    let crate::messaging::Command::Navigation(Command::Navigate {
        message: quoted,
        quote: true,
        request,
        ..
    }) = rx.try_recv().unwrap()
    else {
        panic!("quote navigation expected")
    };
    assert_eq!(quoted, old_target);
    // A missing quote is acknowledged without losing the Back action.
    let missing = Snapshot {
        jump: crate::message_navigation::Jump {
            request,
            browsing: true,
            ..Default::default()
        },
        ..Snapshot::empty()
    };
    publish(&ui, &missing);
    ui.invoke_navigate_message(0, false);
    let crate::messaging::Command::Navigation(Command::Navigate { request, .. }) =
        rx.try_recv().unwrap()
    else {
        panic!("return expected")
    };
    let snapshot = Snapshot {
        jump: crate::message_navigation::Jump {
            request,
            returning: true,
            ..Default::default()
        },
        ..Snapshot::empty()
    };
    prepare(&ui, &snapshot);
    crate::presentation::conversation(&ui, &view, false);
    publish(&ui, &snapshot);
    assert!(!ui.get_browsing_history());
    let position = timeline::position(&ui);
    assert_eq!(old_position.0, position.0);
    assert!(
        (old_position.1 - position.1).abs() < 1.,
        "{old_position:?} != {position:?}"
    );
    // A superseded query or another chat cannot replace the current results.
    ui.set_message_search_open(true);
    ui.set_message_search_query("日本語".into());
    ui.set_message_search_request(8);
    let mut snapshot = Snapshot {
        search: crate::message_navigation::Search {
            chat: ui.get_selected_id(),
            request: 7,
            query: "日本語".into(),
            hits: vec![crate::message_navigation::Hit {
                id: message,
                text: "日本語".into(),
                time: "12:00".into(),
            }],
            more: false,
        },
        ..Snapshot::empty()
    };
    publish(&ui, &snapshot);
    assert_eq!(ui.get_message_search_hits().row_count(), 0);
    snapshot.search.request = 8;
    publish(&ui, &snapshot);
    assert_eq!(ui.get_message_search_hits().row_count(), 1);
    snapshot.search.chat += 1;
    snapshot.search.hits.clear();
    publish(&ui, &snapshot);
    assert_eq!(ui.get_message_search_hits().row_count(), 1);
    for (width, height) in [(1200., 800.), (620., 480.), (360., 480.)] {
        ui.window().set_size(slint::LogicalSize::new(width, height));
        ui.invoke_layout_timeline();
        assert!(ui.get_timeline_visible() > 0.);
    }
    // A chat switch also releases a pending navigation gate.
    ui.invoke_navigate_message(message, false);
    state.select(2);
    crate::presentation::conversation(&ui, &state.conversation(), true);
    assert_eq!(ui.get_navigation_request(), ui.get_navigation_shown());
    assert!(!ui.get_message_search_open());
    ui.hide().unwrap();
}
