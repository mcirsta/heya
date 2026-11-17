//! Shared navigation presentation; storage work is queued, never performed here.
use crate::ui::MessageSearchHit;
use crate::{
    AppWindow, live_presentation, message_navigation::Command, messaging::Snapshot,
    presentation::timeline,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
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
                    if ui.get_message_search_all() {
                        "Whole words · all words must match"
                    } else {
                        "Search this conversation’s saved history"
                    }
                } else {
                    "Searching…"
                }
                .into(),
            );
            let command = Command::Search {
                chat: if ui.get_message_search_all() {
                    0
                } else {
                    ui.get_selected_id()
                },
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
                            && ui.get_message_search_request() == request
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
    ui.on_open_search_result({
        let weak = ui.as_weak();
        let submit = submit.clone();
        move |chat, message| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_navigation_request() != ui.get_navigation_shown() {
                return;
            }
            let request = ui.get_navigation_request().wrapping_add(1).max(1);
            let revision = ui.get_live_revision().wrapping_add(1);
            if submit(
                &ui,
                Command::Open {
                    chat,
                    message,
                    previous: ui.get_selected_id(),
                    draft: ui.get_draft().into(),
                    draft_version: ui.get_draft_revision(),
                    request,
                    revision,
                },
            ) {
                if chat == ui.get_selected_id() && !ui.get_browsing_history() {
                    timeline::save_position(&ui);
                }
                ui.set_navigation_request(request);
                ui.set_live_revision(revision);
                ui.invoke_reveal_chat();
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
    publish_search(ui, &snapshot.search);
    let jump = snapshot.jump;
    if jump.request == ui.get_navigation_request() && jump.request != ui.get_navigation_shown() {
        if (jump.message != 0 || jump.returning)
            && !timeline::navigate(ui, jump.message, jump.returning)
        {
            ui.invoke_show_error(
                "The original position is no longer available in saved history.".into(),
            );
        }
        ui.set_highlighted_message(timeline::display_id(ui, jump.message));
        ui.set_navigation_shown(jump.request);
    }
}

pub(crate) fn publish_search(ui: &AppWindow, search: &crate::message_navigation::Search) {
    if ui.get_message_search_open()
        && search.chat
            == if ui.get_message_search_all() {
                0
            } else {
                ui.get_selected_id()
            }
        && search.request == ui.get_message_search_request()
        && ui.get_message_search_query() == search.query
    {
        ui.set_message_search_busy(search.indexing);
        let rows = search
            .hits
            .iter()
            .map(|h| MessageSearchHit {
                id: h.id,
                chat: h.chat,
                title: h.title.clone().into(),
                text: h.text.clone().into(),
                time: h.time.clone().into(),
            })
            .collect::<Vec<_>>();
        if !ui.get_message_search_hits().iter().eq(rows.iter().cloned()) {
            ui.set_message_search_hits(ModelRc::new(VecModel::from(rows)));
        }
        ui.set_message_search_status(if !search.error.is_empty() {
            search.error.as_str().into()
        } else if search.query.trim().is_empty() {
            if ui.get_message_search_all() {
                "Whole words · all words must match"
            } else {
                "Search this conversation’s saved history"
            }
            .into()
        } else if search.indexing {
            "Preparing saved history for search…".into()
        } else if search.more {
            "Showing 100 matches · add words to narrow your search".into()
        } else if search.hits.is_empty() {
            "No matches in saved history".into()
        } else {
            format!("{} matches in saved history", search.hits.len()).into()
        });
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
                chat: ui.get_selected_id(),
                title: String::new(),
                text: "日本語".into(),
                time: "12:00".into(),
            }],
            more: false,
            ..Default::default()
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
    check_global_search_ui();
}

#[cfg(test)]
fn check_global_search_ui() {
    use crate::message_navigation::{Hit, Jump, Search};
    use i_slint_backend_testing::ElementHandle;
    let ui = AppWindow::new().unwrap();
    ui.set_live_chat_view(true);
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let mut state = crate::fixtures::demo();
    crate::presentation::conversation(&ui, &state.conversation(), true);
    ui.set_draft("Keep this draft".into());
    ui.show().unwrap();
    let (tx, mut rx) = mpsc::channel(1);
    bind_live(&ui, tx);
    ElementHandle::find_by_accessible_label(&ui, "Search in all chats")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(ui.get_message_search_all() && ui.get_message_search_open());
    assert!(matches!(
        rx.try_recv().unwrap(),
        crate::messaging::Command::Navigation(Command::Search { chat: 0, .. })
    ));
    ui.set_message_search_query("meeting".into());
    ui.invoke_search_messages("meeting".into());
    let old = ui.get_message_search_request();
    ui.set_message_search_query("meeting project".into());
    ui.invoke_search_messages("meeting project".into());
    // This aggregate UI test uses the system-time testing backend.
    std::thread::sleep(std::time::Duration::from_millis(300));
    slint::platform::update_timers_and_animations();
    let crate::messaging::Command::Navigation(Command::Search { request, query, .. }) =
        rx.try_recv().unwrap()
    else {
        panic!("global search expected")
    };
    assert_eq!(query, "meeting project");
    assert!(rx.try_recv().is_err(), "superseded input is not submitted");
    let mut search = Search {
        request: old,
        query: "meeting".into(),
        hits: vec![Hit {
            id: 1,
            chat: 2,
            title: "Other chat".into(),
            text: "A meeting for the project".into(),
            time: "12:00".into(),
        }],
        ..Default::default()
    };
    publish_search(&ui, &search);
    assert_eq!(ui.get_message_search_hits().row_count(), 0);
    search.request = request;
    search.query = query;
    publish_search(&ui, &search);
    assert_eq!(ui.get_message_search_hits().row_count(), 1);
    let model = ui.get_message_search_hits();
    publish_search(&ui, &search);
    assert_eq!(
        model,
        ui.get_message_search_hits(),
        "unchanged results reuse their model"
    );
    state.select(2);
    let view = state.conversation();
    let target = view.messages[0].id;
    ui.invoke_open_search_result(2, target);
    ui.invoke_open_search_result(2, target);
    let crate::messaging::Command::Navigation(Command::Open { request, draft, .. }) =
        rx.try_recv().unwrap()
    else {
        panic!("open result expected")
    };
    assert_eq!(draft, "Keep this draft");
    assert!(
        rx.try_recv().is_err(),
        "pending navigation absorbs double clicks"
    );
    let snapshot = Snapshot {
        search: search.clone(),
        jump: Jump {
            request,
            message: target,
            browsing: true,
            ..Default::default()
        },
        ..Snapshot::empty()
    };
    prepare(&ui, &snapshot);
    crate::presentation::conversation(&ui, &view, true);
    publish(&ui, &snapshot);
    assert_eq!(ui.get_selected_id(), 2);
    assert!(ui.get_message_search_open());
    assert_eq!(ui.get_message_search_query(), "meeting project");
    assert_eq!(ui.get_navigation_shown(), request);
    assert_eq!(ui.get_highlighted_message(), target);
    for width in [1200., 620., 360.] {
        ui.window().set_size(slint::LogicalSize::new(width, 800.));
        ui.set_show_conversation(false);
        let field =
            ElementHandle::find_by_accessible_label(&ui, "Search messages across all chats")
                .next()
                .unwrap();
        assert!(field.size().width > 100.);
        assert!(field.absolute_position().x >= 0.);
        assert!(field.absolute_position().x + field.size().width <= width);
    }
    search.error = "Storage is unavailable".into();
    search.hits.clear();
    publish_search(&ui, &search);
    assert!(!ui.get_message_search_busy());
    assert_eq!(ui.get_message_search_status(), "Storage is unavailable");
    ui.invoke_close_message_search();
    assert!(!ui.get_message_search_open());
    // The close command occupies the one-slot queue. A rejected jump must not
    // advance the navigation gate or discard the current draft.
    let request = ui.get_navigation_request();
    let draft = ui.get_draft();
    ui.invoke_open_search_result(2, target);
    assert_eq!(ui.get_navigation_request(), request);
    assert_eq!(ui.get_draft(), draft);
    assert!(!ui.get_error_message().is_empty());
    ui.hide().unwrap();
}
