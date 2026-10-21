//! Bounded UI handoff for explicit message operations. No protocol work here.
use crate::{
    AppWindow,
    message_actions::{Command, Destination, OperationReply},
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{rc::Rc, sync::Arc};

// Shared with MessageBubble/MessageMenu: 0 react, 1 reply, 2 forward,
// 3 open menu, 4 copy, 5 download, 6 pin, 7 star, 8 report, 9 delete, 10 AI.
const MENU: i32 = 3;
const COPY: i32 = 4;
const DOWNLOAD: i32 = 5;

pub fn bind(ui: &AppWindow, submit: impl Fn(Command) -> bool + 'static) {
    crate::presentation_emoji::bind(ui);
    let submit = Rc::new(submit);
    let selection = crate::presentation_message_controls::bind(ui, submit.clone());
    ui.on_search_message_actions({
        let weak = ui.as_weak();
        move |query| {
            if let Some(ui) = weak.upgrade() {
                search(&ui, &query);
            }
        }
    });
    ui.on_message_action({
        let weak = ui.as_weak();
        let submit = submit.clone();
        move |id, action, x, y| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if ui.get_action_pending()
                || ui.get_live_revision() != ui.get_live_shown_revision() && !ui.get_demo_mode()
            {
                return;
            }
            let Some(row) = ui.get_timeline_rows().iter().find(|r| r.message.id == id) else {
                return;
            };
            if !row.message.actionable
                && (!matches!(action, 14 | 15 | 17 | MENU) || row.message.system_notice)
            {
                return;
            }
            ui.set_action_source_actionable(row.message.actionable);
            ui.set_action_error("".into());
            ui.set_action_message(id);
            ui.set_action_chat(ui.get_selected_id());
            ui.set_action_x(x);
            ui.set_action_y(y);
            ui.set_action_summary(
                format!(
                    "{}: {}",
                    row.message.sender,
                    crate::domain::single_line_summary(if row.message.text.is_empty() {
                        if row.message.has_photo {
                            "Photo"
                        } else if row.message.has_audio {
                            "Audio message"
                        } else {
                            "Message"
                        }
                    } else {
                        &row.message.text
                    })
                )
                .into(),
            );
            ui.set_action_own_emoji(
                if row.message.reaction_status.starts_with("Reaction failed") {
                    "".into()
                } else {
                    row.message.own_reaction.clone()
                },
            );
            match action {
                11 => {
                    ui.invoke_select_message(id);
                }
                12 => {
                    ui.set_action_expected_text(row.message.text.clone());
                    ui.set_action_edit_text(row.message.text.clone());
                    ui.invoke_open_message_control(5);
                }
                13 => {
                    ui.invoke_open_message_control(6);
                }
                14 => {
                    ui.invoke_open_message_control(7);
                }
                15 => {
                    ui.set_info_wire("".into());
                    ui.set_info_sent("Loading…".into());
                    ui.set_info_status("".into());
                    ui.set_info_receipts(ModelRc::default());
                    ui.set_message_dialog(8);
                    request(&ui, &submit, |request| Command::Info {
                        chat: ui.get_action_chat(),
                        message: id,
                        request,
                    });
                }
                0 => {
                    ui.set_message_dialog(1);
                    ui.invoke_begin_emoji_picker();
                }
                1 => {
                    if ui.get_demo_mode() || ui.get_live_can_send() {
                        request(&ui, &submit, |request| Command::Reply {
                            chat: ui.get_selected_id(),
                            message: id,
                            request,
                        });
                        ui.invoke_focus_composer();
                    }
                }
                2 => {
                    ui.set_message_dialog(2);
                    ui.set_action_destinations(ModelRc::default());
                    request(&ui, &submit, |request| Command::Destinations { request });
                }
                MENU | 17 => {
                    ui.set_action_context_menu(action == 17);
                    ui.invoke_prepare_quick_reactions(ui.get_action_own_emoji());
                    ui.set_message_menu_entries(ModelRc::new(VecModel::from(menu_entries(
                        &ui,
                        &row.message,
                    ))));
                    ui.set_message_menu_selection(first_menu_selection(&ui));
                    ui.set_message_dialog(MENU);
                }
                _ => {}
            }
        }
    });
    ui.on_choose_message_menu({
        let weak = ui.as_weak();
        move |action| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_message_dialog() != MENU || !current_source(&ui) {
                return;
            }
            let Some(row) = ui
                .get_timeline_rows()
                .iter()
                .find(|r| r.message.id == ui.get_action_message())
            else {
                return;
            };
            // Re-evaluate capabilities at activation: connection and message
            // state may have changed while the menu was open.
            if action != 0
                && !menu_entries(&ui, &row.message)
                    .iter()
                    .any(|e| e.action == action && e.enabled)
            {
                return;
            }
            if action == 16 {
                ui.invoke_copy_value(row.message.wire_id.clone());
                ui.invoke_close_message_actions();
            } else if action == 18 {
                let id = ui.get_action_message();
                ui.invoke_close_message_actions();
                ui.invoke_copy_photo(id);
            } else if action == COPY {
                if let crate::message_content::Target::Copy(value) =
                    crate::presentation::timeline::action(&ui, row.message.id, -6)
                {
                    ui.invoke_copy_value(value.into());
                }
                ui.invoke_close_message_actions();
            } else if action == DOWNLOAD {
                let chat = ui.get_action_chat();
                let message = ui.get_action_message();
                ui.invoke_close_message_actions();
                ui.invoke_download_message(chat, message);
            } else {
                ui.invoke_close_message_actions();
                ui.invoke_message_action(
                    row.message.id,
                    action,
                    ui.get_action_x(),
                    ui.get_action_y(),
                );
            }
        }
    });
    ui.on_navigate_message_menu({
        let weak = ui.as_weak();
        move |direction| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_message_dialog() != MENU {
                return;
            }
            let available = menu_selections(&ui);
            if available.is_empty() {
                return;
            }
            let current = available
                .iter()
                .position(|i| *i == ui.get_message_menu_selection());
            let index = match direction {
                -100 => 0,
                100 => available.len() - 1,
                n if n < 0 => current.map_or(available.len() - 1, |i| {
                    (i + available.len() - 1) % available.len()
                }),
                _ => current.map_or(0, |i| (i + 1) % available.len()),
            };
            ui.set_message_menu_selection(available[index]);
        }
    });
    ui.on_cancel_reply({
        let weak = ui.as_weak();
        let submit = submit.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                request(&ui, &submit, |request| Command::Reply {
                    chat: ui.get_selected_id(),
                    message: 0,
                    request,
                });
            }
        }
    });
    ui.on_react_message({
        let weak = ui.as_weak();
        let submit = submit.clone();
        move |emoji| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !matches!(ui.get_message_dialog(), 1 | MENU)
                || !ready(&ui)
                || (!emoji.is_empty() && emojis::get(&emoji).is_none())
            {
                return;
            }
            let emoji = if emoji == ui.get_action_own_emoji() {
                String::new()
            } else {
                emoji.to_string()
            };
            request(&ui, &submit, |request| Command::React {
                chat: ui.get_action_chat(),
                message: ui.get_action_message(),
                emoji: emoji.clone(),
                request,
            });
            if ui.get_action_pending() && !emoji.is_empty() {
                ui.invoke_remember_reaction(emoji.into());
            }
        }
    });
    ui.on_forward_message({
        let weak = ui.as_weak();
        move |destination| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if ui.get_message_dialog() != 2
                || !ready(&ui)
                || !ui
                    .get_action_all_destinations()
                    .iter()
                    .any(|d| d.id == destination)
            {
                return;
            }
            let messages = selection.forward.borrow().clone();
            if ui.get_selection_active() && !messages.is_empty() {
                request(&ui, &submit, |request| Command::ForwardMany {
                    chat: ui.get_action_chat(),
                    messages,
                    destination,
                    request,
                });
            } else {
                request(&ui, &submit, |request| Command::Forward {
                    chat: ui.get_action_chat(),
                    message: ui.get_action_message(),
                    destination,
                    request,
                });
            }
        }
    });
}
fn ready(ui: &AppWindow) -> bool {
    current_source(ui)
        && (ui.get_selection_active() || ui.get_action_source_actionable())
        && (ui.get_demo_mode() || ui.get_connection_state() == 5)
        && !ui.get_action_pending()
}
fn current_source(ui: &AppWindow) -> bool {
    ui.get_action_chat() == ui.get_selected_id()
        && (ui.get_demo_mode() || ui.get_live_revision() == ui.get_live_shown_revision())
        && ((ui.get_selection_active() && ui.get_selection_count() > 0)
            || ui
                .get_timeline_rows()
                .iter()
                .any(|r| r.message.id == ui.get_action_message() && !r.message.system_notice))
}

fn menu_entries(ui: &AppWindow, row: &crate::MessageRow) -> Vec<crate::ui::MessageMenuEntry> {
    let online = ui.get_demo_mode() || ui.get_connection_state() == 5;
    let idle = !ui.get_action_pending();
    let mut entries = Vec::with_capacity(11);
    let mut offset = 0;
    let mut add = |action, label: &str, enabled, reason: &str, separator| {
        if separator {
            offset += if ui.get_action_context_menu() { 9 } else { 8 };
        }
        entries.push(crate::ui::MessageMenuEntry {
            offset,
            action,
            label: label.into(),
            enabled,
            reason: if enabled { "" } else { reason }.into(),
            separator,
        });
    };
    if !row.actionable {
        add(
            14,
            "Delete for me",
            idle,
            "Please wait for the current action",
            false,
        );
        if row.has_photo {
            add(
                18,
                "Copy image",
                row.photo_state == 2 && !ui.get_clipboard_busy(),
                "Wait for the photo to finish loading",
                false,
            );
        }
        if row.outgoing {
            add(
                15,
                "Message info",
                idle,
                "Please wait for the current action",
                true,
            );
        }
        add(
            16,
            "Copy message ID",
            !row.wire_id.is_empty(),
            "The message ID is unavailable",
            !row.outgoing,
        );
        return entries;
    }
    if ui.get_action_context_menu() {
        let now = chrono::Utc::now().timestamp();
        let stamp = row.sent_at.parse().unwrap_or(0);
        let editable = row.outgoing
            && !row.has_attachment
            && row.actions.row_count() == 0
            && crate::message_actions::within_window(
                stamp,
                now,
                crate::message_actions::EDIT_WINDOW,
            );
        let revoke = row.outgoing
            && crate::message_actions::within_window(
                stamp,
                now,
                crate::message_actions::REVOKE_WINDOW,
            );
        add(
            1,
            "Reply",
            idle && (ui.get_demo_mode() || ui.get_live_can_send()),
            "Reply is unavailable in this conversation",
            false,
        );
        add(2, "Forward", idle && online, "Connect to forward", false);
        add(
            11,
            "Select",
            idle,
            "Please wait for the current action",
            false,
        );
        if !row.text.is_empty() {
            add(COPY, "Copy text", true, "", false);
        }
        if editable {
            add(
                12,
                "Edit",
                online && idle && !row.operation_busy,
                "Connect, or wait for the pending message operation",
                false,
            );
        }
        if revoke {
            add(
                13,
                "Delete for everyone",
                online && idle && !row.operation_busy,
                "Connect, or wait for the pending message operation",
                false,
            );
        }
        add(
            14,
            "Delete for me",
            idle,
            "Please wait for the current action",
            false,
        );
        if row.has_attachment {
            add(
                DOWNLOAD,
                "Download",
                !(1..=3).contains(&ui.get_download_phase()),
                "Wait for the current download or cancel it",
                false,
            );
        }
        if row.outgoing {
            add(
                15,
                "Message info",
                idle,
                "Please wait for the current action",
                true,
            );
        }
        add(
            16,
            "Copy message ID",
            !row.wire_id.is_empty(),
            "The message ID is unavailable",
            !row.outgoing,
        );
        return entries;
    }
    add(
        1,
        "Reply",
        idle && (ui.get_demo_mode() || ui.get_live_can_send()),
        "Reply is unavailable in this conversation",
        false,
    );
    // The picker can open offline; only sending a reaction needs connection.
    add(
        0,
        "React",
        idle,
        "Please wait for the current action",
        false,
    );
    if row.has_attachment {
        add(
            5,
            "Download",
            !(1..=3).contains(&ui.get_download_phase()),
            "Wait for the current download or cancel it",
            false,
        );
    }
    if row.has_photo {
        add(
            18,
            "Copy image",
            row.photo_state == 2 && !ui.get_clipboard_busy(),
            "Wait for the photo to finish loading",
            false,
        );
    }
    if !row.text.is_empty() && (!row.has_attachment || row.has_photo || row.has_document) {
        add(
            COPY,
            if row.has_photo || row.has_document {
                "Copy caption"
            } else {
                "Copy"
            },
            true,
            "",
            false,
        );
    }
    add(2, "Forward", idle && online, "Connect to forward", false);
    let stamp = row.sent_at.parse().unwrap_or(0);
    let now = chrono::Utc::now().timestamp();
    if row.outgoing
        && !row.has_attachment
        && row.actions.row_count() == 0
        && crate::message_actions::within_window(stamp, now, crate::message_actions::EDIT_WINDOW)
    {
        add(
            12,
            "Edit",
            online && idle && !row.operation_busy,
            "Connect, or wait for the pending message operation",
            false,
        );
    }
    if row.outgoing
        && crate::message_actions::within_window(stamp, now, crate::message_actions::REVOKE_WINDOW)
    {
        add(
            13,
            "Delete for everyone",
            online && idle && !row.operation_busy,
            "Connect, or wait for the pending message operation",
            false,
        );
    }
    add(6, "Pin", false, "Not implemented yet", false);
    add(10, "Ask Meta AI", false, "Not available in Heya", false);
    add(7, "Star", false, "Not implemented yet", false);
    if !row.outgoing {
        add(8, "Report", false, "Not implemented yet", true);
    }
    add(
        14,
        "Delete for me",
        idle,
        "Please wait for the current action",
        row.outgoing,
    );
    if row.outgoing {
        add(
            15,
            "Message info",
            idle,
            "Please wait for the current action",
            true,
        );
    }
    add(
        16,
        "Copy message ID",
        !row.wire_id.is_empty(),
        "The message ID is unavailable",
        !row.outgoing,
    );
    entries
}

fn menu_selections(ui: &AppWindow) -> Vec<i32> {
    let online = ui.get_action_source_actionable()
        && !ui.get_action_pending()
        && (ui.get_demo_mode() || ui.get_connection_state() == 5);
    let n = reaction_count(ui);
    (if online { 0 } else { n - 1 }..n)
        .chain(
            ui.get_message_menu_entries()
                .iter()
                .enumerate()
                .filter(|(_, e)| e.enabled)
                .map(|(i, _)| i as i32 + n),
        )
        .collect()
}
fn first_menu_selection(ui: &AppWindow) -> i32 {
    menu_selections(ui)
        .into_iter()
        .find(|i| *i >= reaction_count(ui))
        .unwrap_or(reaction_count(ui) - 1)
}
fn reaction_count(ui: &AppWindow) -> i32 {
    if ui.get_action_context_menu() {
        ui.get_menu_reactions().row_count() as i32
    } else {
        7
    }
}
pub(crate) fn request(
    ui: &AppWindow,
    submit: &Rc<impl Fn(Command) -> bool>,
    build: impl FnOnce(i32) -> Command,
) {
    if ui.get_action_pending() {
        return;
    }
    let id = ui.get_action_request().wrapping_add(1);
    if submit(build(id)) {
        ui.set_action_request(id);
        ui.set_action_pending(true);
        ui.set_action_error("".into());
    } else {
        ui.set_action_error("Heya is busy. Please try again.".into());
        if ui.get_message_dialog() == MENU {
            ui.set_notice(ui.get_action_error());
            ui.invoke_close_message_actions();
        }
    }
}
fn search(ui: &AppWindow, query: &str) {
    let query = query.chars().take(128).collect::<String>().to_lowercase();
    if matches!(ui.get_message_dialog(), 1 | 4) {
        ui.set_emoji_query(query.into());
        ui.invoke_request_emojis();
    } else {
        let values = ui
            .get_action_all_destinations()
            .iter()
            .filter(|d| d.title.to_lowercase().contains(&query))
            .collect::<Vec<_>>();
        ui.set_action_destinations(ModelRc::new(VecModel::from(values)));
    }
}
pub fn publish(ui: &AppWindow, result: Option<&OperationReply>, destinations: &Arc<[Destination]>) {
    if let Some(result) = result.filter(|r| r.kind == 7 && r.request == ui.get_action_request()) {
        ui.set_action_pending(false);
        if ui.get_message_dialog() == 8 {
            ui.set_action_error(result.error.as_str().into());
            if let Some(info) = &result.info {
                if info.message != ui.get_action_message() {
                    return;
                }
                ui.set_info_wire(info.wire.as_str().into());
                ui.set_info_sent(info.sent.as_str().into());
                ui.set_info_status(info.status.as_str().into());
                let rows: Vec<crate::ui::MessageInfoRow> = info
                    .rows
                    .iter()
                    .map(|r| crate::ui::MessageInfoRow {
                        name: r.name.as_str().into(),
                        detail: r.detail.as_str().into(),
                    })
                    .collect();
                if ui.get_info_receipts().iter().ne(rows.iter().cloned()) {
                    ui.set_info_receipts(ModelRc::new(VecModel::from(rows)));
                }
            }
        }
        return;
    }
    let Some(result) =
        result.filter(|r| r.request == ui.get_action_request() && ui.get_action_pending())
    else {
        return;
    };
    ui.set_action_pending(false);
    if !result.saved {
        ui.set_action_error(result.error.as_str().into());
        ui.set_notice(result.error.as_str().into());
        if ui.get_message_dialog() == MENU {
            ui.invoke_close_message_actions();
        }
        return;
    }
    if result.kind == 3 {
        ui.set_action_all_destinations(ModelRc::new(VecModel::from(
            destinations
                .iter()
                .map(|d| crate::ui::ForwardDestination {
                    id: d.id,
                    title: d.title.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )));
        search(ui, "");
    } else if matches!(result.kind, 1 | 2 | 4 | 5 | 6) {
        ui.invoke_close_message_actions();
        if matches!(result.kind, 2 | 6) && ui.get_selection_active() {
            ui.invoke_selection_control(0);
        }
        ui.set_notice(
            if ui.get_demo_mode() {
                "Updated locally in the demo"
            } else if result.kind == 1 {
                "Reaction queued"
            } else if result.kind == 2 {
                "Forward queued"
            } else if result.kind == 4 {
                "Edit queued"
            } else if result.kind == 5 {
                "Delete queued"
            } else {
                "Deleted for you · linked-device sync queued"
            }
            .into(),
        );
    }
}
