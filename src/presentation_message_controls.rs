//! Window-level selection and edit/delete dialogs; no I/O or row-local workers.
use crate::{
    AppWindow,
    message_actions::{Command, MAX_SELECTION},
};
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, collections::HashSet, rc::Rc};

#[derive(Default)]
pub struct Selection {
    ids: RefCell<HashSet<i32>>,
    targets: RefCell<Vec<i32>>,
    pub forward: RefCell<Vec<i32>>,
}
impl Selection {
    fn refresh(&self, ui: &AppWindow) {
        ui.set_selection_count(self.ids.borrow().len() as i32);
        ui.set_selection_revision(ui.get_selection_revision().wrapping_add(1));
    }
}
pub fn bind(ui: &AppWindow, submit: Rc<impl Fn(Command) -> bool + 'static>) -> Rc<Selection> {
    let state = Rc::new(Selection::default());
    let weak = ui.as_weak();
    let selected = state.clone();
    ui.on_message_selected(move |id, _| {
        weak.upgrade().is_some() && selected.ids.borrow().contains(&id)
    });
    let weak = ui.as_weak();
    let selected = state.clone();
    ui.on_select_message(move |id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_message_dialog() != 0
            || !ui
                .get_timeline_rows()
                .iter()
                .any(|r| r.message.id == id && r.message.actionable)
        {
            return;
        }
        let mut ids = selected.ids.borrow_mut();
        if !ids.remove(&id) {
            if ids.len() >= MAX_SELECTION {
                ui.set_notice(format!("Select up to {MAX_SELECTION} messages.").into());
                return;
            }
            ids.insert(id);
        }
        drop(ids);
        ui.set_selection_active(true);
        selected.refresh(&ui);
    });
    let weak = ui.as_weak();
    let selected = state.clone();
    let sender = submit.clone();
    ui.on_selection_control(move |action| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if action == 0 {
            selected.ids.borrow_mut().clear();
            selected.forward.borrow_mut().clear();
            ui.set_selection_active(false);
            selected.refresh(&ui);
            return;
        }
        if action == 4 {
            let valid = crate::presentation::timeline::selected_ids(&ui, &selected.ids.borrow());
            let valid = valid.into_iter().collect();
            if *selected.ids.borrow() != valid {
                *selected.ids.borrow_mut() = valid;
                selected.refresh(&ui);
            }
            return;
        }
        if ui.get_action_pending() || ui.get_message_dialog() != 0 {
            return;
        }
        let ids = crate::presentation::timeline::selected_ids(&ui, &selected.ids.borrow());
        if ids.is_empty() {
            return;
        }
        ui.set_action_chat(ui.get_selected_id());
        ui.set_action_message(ids[0]);
        ui.set_action_error("".into());
        match action {
            1 => {
                ui.invoke_copy_value(
                    crate::presentation::timeline::selected_text(&ui, &selected.ids.borrow())
                        .into(),
                );
            }
            2 => {
                if !ui.get_demo_mode() && ui.get_connection_state() != 5 {
                    ui.set_notice("Connect to forward messages.".into());
                    return;
                }
                ui.set_action_summary(format!("{} selected messages", ids.len()).into());
                *selected.forward.borrow_mut() = ids;
                ui.set_message_dialog(2);
                crate::presentation_actions::request(&ui, &sender, |request| {
                    Command::Destinations { request }
                });
            }
            3 => {
                *selected.targets.borrow_mut() = ids;
                ui.set_control_count(selected.targets.borrow().len() as i32);
                ui.set_message_dialog(7);
            }
            _ => {}
        }
    });
    let weak = ui.as_weak();
    let selected = state.clone();
    ui.on_open_message_control(move |mode| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        *selected.targets.borrow_mut() = vec![ui.get_action_message()];
        ui.set_control_count(1);
        ui.set_message_dialog(mode);
    });
    let weak = ui.as_weak();
    let selected = state.clone();
    let sender = submit.clone();
    ui.on_confirm_message_control(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_action_pending()
            || ui.get_action_chat() != ui.get_selected_id()
            || (!ui.get_demo_mode() && ui.get_live_revision() != ui.get_live_shown_revision())
        {
            return;
        }
        let mode = ui.get_message_dialog();
        if matches!(mode, 5 | 6) && !ui.get_demo_mode() && ui.get_connection_state() != 5 {
            ui.set_action_error("Connect to complete this action.".into());
            return;
        }
        let chat = ui.get_action_chat();
        let message = ui.get_action_message();
        match mode {
            5 => {
                let text = ui.get_action_edit_text().to_string();
                if text.trim().is_empty() || text.len() > 64 * 1024 {
                    ui.set_action_error("Enter a message of up to 64 KiB.".into());
                    return;
                }
                crate::presentation_actions::request(&ui, &sender, |request| Command::Edit {
                    chat,
                    message,
                    expected: ui.get_action_expected_text().into(),
                    text,
                    request,
                });
            }
            6 => crate::presentation_actions::request(&ui, &sender, |request| Command::Revoke {
                chat,
                message,
                request,
            }),
            7 => {
                let messages = selected.targets.borrow().clone();
                crate::presentation_actions::request(&ui, &sender, |request| Command::Hide {
                    chat,
                    messages,
                    request,
                });
            }
            _ => {}
        }
    });
    ui.on_close_message_info(move || {
        let _ = submit(Command::CloseInfo);
    });
    state
}

#[cfg(test)]
#[path = "presentation_message_control_tests.rs"]
pub(crate) mod tests;
