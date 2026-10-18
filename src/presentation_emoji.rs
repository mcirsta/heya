//! One shared, row-virtualized emoji picker for reactions and composer insertion.
use crate::{
    AppWindow,
    ui::{EmojiCategory, EmojiChoice, EmojiGridRow},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

const GROUPS: [(emojis::Group, &str, &str); 9] = [
    (emojis::Group::SmileysAndEmotion, "😀", "Smileys & Emotion"),
    (emojis::Group::PeopleAndBody, "👋", "People & Body"),
    (emojis::Group::AnimalsAndNature, "🐻", "Animals & Nature"),
    (emojis::Group::FoodAndDrink, "🍔", "Food & Drink"),
    (emojis::Group::TravelAndPlaces, "🚗", "Travel & Places"),
    (emojis::Group::Activities, "⚽", "Activities"),
    (emojis::Group::Objects, "💡", "Objects"),
    (emojis::Group::Symbols, "🔣", "Symbols"),
    (emojis::Group::Flags, "🏁", "Flags"),
];

fn available(emoji: &emojis::Emoji) -> bool {
    #[cfg(target_os = "macos")]
    return crate::macos::supports_emoji(emoji.as_str());
    #[cfg(target_os = "linux")]
    return crate::linux_emoji::supports_emoji(emoji.as_str());
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = emoji;
        true
    }
}
#[derive(Default)]
struct Picker {
    recent: Vec<String>,
    frequent: Vec<(String, u32)>,
    opened: Vec<String>,
    choices: Vec<(&'static emojis::Emoji, i32)>,
    starts: Vec<(i32, i32)>,
}
impl Picker {
    fn remember(&mut self, value: &str) {
        self.recent.retain(|s| s != value);
        self.recent.insert(0, value.into());
        self.recent.truncate(40);
        if let Some((_, count)) = self.frequent.iter_mut().find(|(s, _)| s == value) {
            *count = count.saturating_add(1);
        } else {
            self.frequent.push((value.into(), 1));
        }
        self.frequent.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        self.frequent.truncate(40);
    }
    fn build(&mut self, ui: &AppWindow) {
        #[cfg(target_os = "linux")]
        let font_available = crate::linux_emoji::font_available();
        #[cfg(not(target_os = "linux"))]
        let font_available = true;
        self.build_with_font(ui, font_available);
    }
    fn build_with_font(&mut self, ui: &AppWindow, font_available: bool) {
        ui.set_emoji_font_missing(!font_available);
        self.choices.clear();
        self.starts.clear();
        if !font_available {
            ui.set_emoji_rows(ModelRc::default());
            ui.set_emoji_categories(ModelRc::default());
            ui.set_emoji_selection(-1);
            return;
        }
        let query = ui
            .get_emoji_query()
            .chars()
            .take(128)
            .collect::<String>()
            .trim()
            .to_lowercase();
        let columns = ui.get_emoji_columns().clamp(1, 10) as usize;
        let mut rows = Vec::new();
        let mut section = |heading: &str, category: i32, values: Vec<&'static emojis::Emoji>| {
            self.starts.push((category, rows.len() as i32));
            if !heading.is_empty() {
                rows.push(EmojiGridRow {
                    heading: heading.into(),
                    category,
                    cells: ModelRc::default(),
                });
            }
            for chunk in values.chunks(columns) {
                let row = rows.len() as i32;
                let cells = chunk
                    .iter()
                    .map(|e| {
                        let index = self.choices.len() as i32;
                        self.choices.push((e, row));
                        EmojiChoice {
                            emoji: e.as_str().into(),
                            name: e.name().into(),
                            index,
                        }
                    })
                    .collect::<Vec<_>>();
                rows.push(EmojiGridRow {
                    heading: "".into(),
                    category,
                    cells: ModelRc::new(VecModel::from(cells)),
                });
            }
        };
        if !query.is_empty() {
            let found = emojis::iter()
                .filter(|e| available(e))
                .filter(|e| {
                    e.as_str() == query
                        || e.name().contains(&query)
                        || e.shortcodes().any(|s| s.contains(&query))
                })
                .collect::<Vec<_>>();
            section(
                if found.is_empty() {
                    "Nothing matches"
                } else {
                    ""
                },
                -1,
                found,
            );
        } else {
            let recent = self
                .opened
                .iter()
                .filter_map(|s| emojis::get(s))
                .filter(|e| available(e))
                .collect::<Vec<_>>();
            if !recent.is_empty() {
                section(
                    if ui.get_message_dialog() == 4 {
                        "Recent"
                    } else {
                        "Frequently Used"
                    },
                    0,
                    recent,
                );
            }
            for (i, (group, _, label)) in GROUPS.iter().enumerate() {
                section(
                    label,
                    i as i32 + 1,
                    group.emojis().filter(|e| available(e)).collect(),
                );
            }
        }
        let mut categories = Vec::new();
        if !self.opened.is_empty() {
            categories.push(EmojiCategory {
                emoji: "🕒".into(),
                name: if ui.get_message_dialog() == 4 {
                    "Recent"
                } else {
                    "Frequently Used"
                }
                .into(),
                index: 0,
            });
        }
        categories.extend(
            GROUPS
                .iter()
                .enumerate()
                .map(|(i, (_, icon, name))| EmojiCategory {
                    emoji: (*icon).into(),
                    name: (*name).into(),
                    index: i as i32 + 1,
                }),
        );
        ui.set_emoji_categories(ModelRc::new(VecModel::from(categories)));
        ui.set_emoji_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_emoji_selection(0);
        ui.set_emoji_scroll_row(0);
        ui.set_emoji_jump_revision(ui.get_emoji_jump_revision().wrapping_add(1));
    }
}

pub fn bind(ui: &AppWindow) {
    let state = Rc::new(RefCell::new(Picker::default()));
    ui.on_prepare_quick_reactions({
        let weak = ui.as_weak();
        let state = state.clone();
        move |own| {
            if let Some(ui) = weak.upgrade() {
                let s = state.borrow();
                let mut values: Vec<slint::SharedString> = Vec::with_capacity(8);
                for e in s
                    .frequent
                    .iter()
                    .map(|(e, _)| e.as_str())
                    .chain(["👍", "❤️", "😂", "😮", "😢", "🙏"])
                {
                    if emojis::get(e).is_some() && !values.iter().any(|v| v == e) {
                        values.push(e.into());
                    }
                    if values.len() == 6 {
                        break;
                    }
                }
                if emojis::get(&own).is_some() && !values.contains(&own) {
                    values.push(own);
                }
                values.push("+".into());
                ui.set_menu_reactions(ModelRc::new(VecModel::from(values)));
            }
        }
    });
    ui.on_remember_reaction({
        let state = state.clone();
        move |e| {
            if emojis::get(&e).is_some() {
                state.borrow_mut().remember(&e);
            }
        }
    });
    ui.on_begin_emoji_picker({
        let state = state.clone();
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                let mut s = state.borrow_mut();
                s.opened = if ui.get_message_dialog() == 4 {
                    s.recent.clone()
                } else {
                    s.frequent.iter().map(|(e, _)| e.clone()).collect()
                };
                ui.set_emoji_query("".into());
                s.build(&ui);
            }
        }
    });
    ui.on_request_emojis({
        let state = state.clone();
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade()
                && matches!(ui.get_message_dialog(), 1 | 4)
            {
                state.borrow_mut().build(&ui);
            }
        }
    });
    ui.on_open_composer_emoji({
        let weak = ui.as_weak();
        move |x, y| {
            if let Some(ui) = weak.upgrade()
                && ui.get_selected_id() > 0
                && !ui.get_action_pending()
                && (ui.get_demo_mode() || ui.get_live_revision() == ui.get_live_shown_revision())
            {
                if ui.get_message_dialog() == 4 {
                    ui.invoke_close_message_actions();
                    return;
                }
                ui.set_action_chat(ui.get_selected_id());
                ui.set_action_x(x);
                ui.set_action_y(y);
                ui.set_action_error("".into());
                ui.set_message_dialog(4);
                ui.invoke_begin_emoji_picker();
            }
        }
    });
    ui.on_choose_emoji({
        let weak = ui.as_weak();
        let state = state.clone();
        move |value| {
            if let Some(ui) = weak.upgrade() {
                if emojis::get(&value).is_none() {
                    return;
                }
                let picked = state
                    .borrow()
                    .choices
                    .iter()
                    .position(|(e, _)| e.as_str() == value.as_str());
                if let Some(index) = picked {
                    let row = state.borrow().choices[index].1;
                    ui.set_emoji_scroll_row(row);
                    ui.set_emoji_selection(index as i32);
                }
                if ui.get_message_dialog() == 4 {
                    if ui.get_action_chat() != ui.get_selected_id() {
                        return;
                    }
                    ui.invoke_insert_draft_emoji(value.clone());
                    state.borrow_mut().remember(&value);
                } else {
                    ui.invoke_react_message(value.clone());
                }
            }
        }
    });
    ui.on_jump_emoji_category({
        let weak = ui.as_weak();
        let state = state.clone();
        move |category| {
            if let Some(ui) = weak.upgrade() {
                let mut s = state.borrow_mut();
                if !ui.get_emoji_query().is_empty() {
                    ui.set_emoji_query("".into());
                    s.build(&ui);
                }
                if let Some((_, row)) = s.starts.iter().find(|(c, _)| *c == category) {
                    ui.set_emoji_scroll_row(*row);
                    if let Some(index) = s.choices.iter().position(|(_, r)| r >= row) {
                        ui.set_emoji_selection(index as i32);
                    }
                    ui.set_emoji_jump_revision(ui.get_emoji_jump_revision().wrapping_add(1));
                }
            }
        }
    });
    ui.on_navigate_emoji({
        let weak = ui.as_weak();
        move |step| {
            if let Some(ui) = weak.upgrade() {
                let s = state.borrow();
                if s.choices.is_empty() {
                    return;
                }
                let index =
                    (ui.get_emoji_selection() + step).clamp(0, s.choices.len() as i32 - 1) as usize;
                let (emoji, row) = s.choices[index];
                if step == 0 {
                    drop(s);
                    ui.invoke_choose_emoji(emoji.as_str().into());
                } else {
                    ui.set_emoji_scroll_row(row);
                    ui.set_emoji_selection(index as i32);
                }
            }
        }
    });
    ui.on_insert_composer_emoji({
        let weak = ui.as_weak();
        move |value, anchor, cursor| {
            if let Some(ui) = weak.upgrade() {
                if ui.get_message_dialog() != 4 || ui.get_action_chat() != ui.get_selected_id() {
                    return;
                }
                let mut draft = ui.get_draft().to_string();
                let (a, b) = (
                    anchor.min(cursor).max(0) as usize,
                    anchor.max(cursor).max(0) as usize,
                );
                if b > draft.len()
                    || !draft.is_char_boundary(a)
                    || !draft.is_char_boundary(b)
                    || draft.len() + value.len() - (b - a) > 64 * 1024
                {
                    return;
                }
                draft.replace_range(a..b, &value);
                ui.set_draft(draft.clone().into());
                ui.invoke_draft_edited(draft.into());
                ui.invoke_set_composer_cursor((a + value.len()) as i32);
            }
        }
    });
}

#[cfg(test)]
pub(crate) fn check_missing_font(ui: &AppWindow) {
    use i_slint_backend_testing::ElementHandle;
    use slint::Model;
    let mut picker = Picker::default();
    for (width, height, query) in [(1200., 900., ""), (520., 480., "smile")] {
        ui.window().set_size(slint::LogicalSize::new(width, height));
        ui.set_emoji_query(query.into());
        picker.build_with_font(ui, false);
        assert!(ui.get_emoji_font_missing());
        assert_eq!(ui.get_emoji_rows().row_count(), 0);
        assert_eq!(ui.get_emoji_categories().row_count(), 0);
        assert!(picker.choices.is_empty() && picker.starts.is_empty());
        let panel = ElementHandle::find_by_element_id(ui, "EmojiPicker::panel")
            .next()
            .unwrap();
        let notice = ElementHandle::find_by_element_id(ui, "EmojiPicker::font-notice")
            .next()
            .unwrap();
        let p = panel.absolute_position();
        let n = notice.absolute_position();
        assert!(notice.size().height > 30.);
        assert!(n.x >= p.x && n.y >= p.y);
        assert!(n.x + notice.size().width <= p.x + panel.size().width);
        assert!(n.y + notice.size().height <= p.y + panel.size().height);
    }
    ui.window().set_size(slint::LogicalSize::new(1200., 900.));
    ui.set_emoji_query("".into());
    picker.build(ui);
    assert!(!ui.get_emoji_font_missing());
    assert!(ui.get_emoji_rows().row_count() > 0);
}
