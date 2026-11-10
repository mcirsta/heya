//! Poll input uses the shared bounded action channel; results belong to the archive.
use crate::{
    AppWindow,
    message_actions::{Command, OperationReply},
    ui::{PollDraftOption, PollOptionRow, PollVoterRow},
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;
pub fn bind(ui: &AppWindow, submit: Rc<impl Fn(Command) -> bool + 'static>) {
    let weak = ui.as_weak();
    ui.on_open_create_poll(move || {
        let Some(ui) = weak.upgrade() else { return };
        if !ui.invoke_can_import() || ui.get_action_pending() {
            return;
        }
        if !ui.get_demo_mode()
            && (ui.get_connection_state() != 5
                || ui.get_live_revision() != ui.get_live_shown_revision())
        {
            ui.invoke_show_error("Connect to WhatsApp to create a poll.".into());
            return;
        }
        ui.set_poll_question("".into());
        ui.set_poll_multiple(true);
        ui.set_poll_draft_options(ModelRc::new(VecModel::from(vec![
            PollDraftOption::default();
            2
        ])));
        ui.set_poll_create_chat(ui.get_selected_id());
        ui.set_poll_create_reply(ui.get_reply_id());
        ui.set_action_error("".into());
        ui.set_message_dialog(10);
    });
    let weak = ui.as_weak();
    ui.on_add_poll_option(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut rows = ui.get_poll_draft_options().iter().collect::<Vec<_>>();
        if ui.get_action_pending() || rows.len() >= 12 {
            return;
        }
        rows.push(PollDraftOption::default());
        ui.set_poll_draft_options(ModelRc::new(VecModel::from(rows)));
    });
    let weak = ui.as_weak();
    ui.on_remove_poll_option(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let mut rows = ui.get_poll_draft_options().iter().collect::<Vec<_>>();
        if ui.get_action_pending() || rows.len() <= 2 || index < 0 || index as usize >= rows.len() {
            return;
        }
        rows.remove(index as usize);
        ui.set_poll_draft_options(ModelRc::new(VecModel::from(rows)));
    });
    let weak = ui.as_weak();
    let send = submit.clone();
    ui.on_send_poll(move || {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_message_dialog() != 10
            || ui.get_action_pending()
            || ui.get_poll_create_chat() != ui.get_selected_id()
        {
            return;
        }
        let draft = crate::polls::Draft {
            question: ui.get_poll_question().trim().into(),
            options: ui
                .get_poll_draft_options()
                .iter()
                .map(|r| r.text.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            multiple: ui.get_poll_multiple(),
        };
        if !draft.valid() {
            ui.set_action_error(
                "Enter a question and 2–12 different options (up to 100 characters each).".into(),
            );
            return;
        }
        if !ui.get_demo_mode() && ui.get_connection_state() != 5 {
            ui.set_action_error("Connect to WhatsApp to send this poll.".into());
            return;
        }
        crate::presentation_actions::request(&ui, &send, |request| Command::CreatePoll {
            chat: ui.get_poll_create_chat(),
            draft,
            reply: ui.get_poll_create_reply(),
            request,
        });
    });
    let weak = ui.as_weak();
    let send = submit.clone();
    ui.on_vote_poll(move |id, index| {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_action_pending() || index < 0 {
            return;
        }
        let Some(row) = crate::presentation_actions::source_row(&ui, id).filter(|r| {
            r.has_poll
                && r.poll_can_vote
                && !r.poll_busy
                && r.actionable
                && (index as usize) < r.poll_options.row_count()
        }) else {
            return;
        };
        if !ui.get_demo_mode()
            && (ui.get_connection_state() != 5
                || ui.get_live_revision() != ui.get_live_shown_revision())
        {
            ui.invoke_show_error("Connect to WhatsApp to vote.".into());
            return;
        }
        crate::presentation_actions::request(&ui, &send, |request| Command::VotePoll {
            chat: ui.get_selected_id(),
            message: row.id,
            option: index as usize,
            request,
        });
    });
    let weak = ui.as_weak();
    ui.on_show_poll_results(move |id, option, limit| {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_action_pending() || option < 0 {
            return;
        }
        let already = ui.get_message_dialog() == 11 && ui.get_action_message() == id;
        if !already
            && !crate::presentation_actions::source_row(&ui, id)
                .is_some_and(|r| r.has_poll && r.poll_can_view)
        {
            return;
        }
        crate::presentation_actions::request(&ui, &submit, |request| Command::PollResults {
            chat: ui.get_selected_id(),
            message: id,
            option: option as usize,
            limit: limit.max(1) as usize,
            request,
        });
        if ui.get_action_pending() {
            ui.set_action_message(id);
            ui.set_action_chat(ui.get_selected_id());
            ui.set_poll_result_limit(limit.max(1));
            ui.set_message_dialog(11);
            if !already {
                ui.set_poll_result_options(Default::default());
                ui.set_poll_result_voters(Default::default());
                ui.set_poll_result_question("".into());
            }
        }
    });
}
pub fn publish(ui: &AppWindow, result: &OperationReply) -> bool {
    if !(12..=14).contains(&result.kind) {
        return false;
    }
    if result.request != ui.get_action_request() {
        return true;
    }
    if result.kind == 14 {
        ui.set_action_pending(false);
        if ui.get_message_dialog() != 11 {
            return true;
        }
        ui.set_action_error(result.error.as_str().into());
        if let Some(data) = result
            .poll_results
            .as_ref()
            .filter(|r| r.message == ui.get_action_message())
        {
            ui.set_poll_result_question(data.question.as_str().into());
            ui.set_poll_result_option(data.option as i32);
            ui.set_poll_result_more(data.more);
            let options = data
                .options
                .iter()
                .enumerate()
                .map(|(i, s)| PollOptionRow {
                    label: s.as_str().into(),
                    count: data.counts.get(i).copied().unwrap_or_default(),
                    selected: i == data.option,
                })
                .collect::<Vec<_>>();
            if ui
                .get_poll_result_options()
                .iter()
                .ne(options.iter().cloned())
            {
                ui.set_poll_result_options(ModelRc::new(VecModel::from(options)));
            }
            let voters = data
                .voters
                .iter()
                .map(|v| PollVoterRow {
                    jid: v.jid.as_str().into(),
                    name: v.name.as_str().into(),
                    initials: crate::domain::initials(&v.name).into(),
                    detail: v.detail.as_str().into(),
                })
                .collect::<Vec<_>>();
            if ui
                .get_poll_result_voters()
                .iter()
                .ne(voters.iter().cloned())
            {
                ui.set_poll_result_voters(ModelRc::new(VecModel::from(voters)));
            }
            ui.invoke_request_avatars();
        } else {
            ui.set_poll_result_voters(Default::default());
            ui.set_poll_result_options(Default::default());
            ui.set_poll_result_question("".into());
        }
        return true;
    }
    if !ui.get_action_pending() {
        return true;
    }
    ui.set_action_pending(false);
    if !result.saved {
        ui.set_action_error(result.error.as_str().into());
        if !matches!(ui.get_message_dialog(), 10 | 11) {
            ui.invoke_show_error(result.error.as_str().into());
        }
    } else if result.kind == 12
        && ui.get_message_dialog() == 10
        && ui.get_poll_create_chat() == ui.get_selected_id()
    {
        ui.set_message_dialog(0);
        ui.invoke_focus_composer();
    }
    true
}

#[cfg(test)]
#[path = "presentation_poll_tests.rs"]
pub(crate) mod tests;
