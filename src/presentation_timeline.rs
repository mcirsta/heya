//! A virtual transcript with cached toolkit measurements and an identity anchor.
//!
//! Only the viewport and a small overscan are measured/materialized. The
//! height index keeps seeking logarithmic; archived rows never become widgets.
#[cfg(test)]
#[path = "presentation_history_tests.rs"]
pub(crate) mod history_tests;
use crate::{AppWindow, MessageRow, TimelineRow, domain};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::Instant,
};

#[derive(Default)]
pub(crate) struct LayoutSamples {
    pub milliseconds: Vec<f64>,
    pub measured_rows: usize,
}
thread_local! {
    static SAMPLES: RefCell<Option<LayoutSamples>> = const { RefCell::new(None) };
}
pub(crate) fn take_samples() -> LayoutSamples {
    SAMPLES.with_borrow_mut(|samples| {
        samples
            .replace(LayoutSamples::default())
            .unwrap_or_default()
    })
}

/// Reuse the height index and exact cached prefix, including date chips. Native
/// right clicks are intercepted once at the window boundary, over child media
/// as well as text; this never measures or constructs another timeline row.
pub(crate) fn context_target(ui: &AppWindow, y: f32) -> Option<i32> {
    let model = ui.get_messages();
    let timeline = model.as_any().downcast_ref::<Timeline>()?;
    let s = timeline.state.borrow();
    let y = f64::from(y - ui.get_timeline_top() - ui.get_timeline_y());
    let i = s.heights.row_at(y);
    let row = s.source.get(i)?;
    let cache = s.cache.get(i)?;
    let top = s.heights.prefix(i) + cache.prefix?;
    (y >= top && y < top + f64::from(cache.body_height) && row.content.notice.is_none())
        .then_some(row.id)
}

#[derive(Default)]
struct Heights {
    values: Vec<f64>,
    tree: Vec<f64>,
}

impl Heights {
    fn new(values: Vec<f64>) -> Self {
        let mut result = Self {
            tree: vec![0.; values.len() + 1],
            values,
        };
        for i in 1..result.tree.len() {
            result.tree[i] += result.values[i - 1];
            let parent = i + i.isolate_lowest_one();
            if parent < result.tree.len() {
                result.tree[parent] += result.tree[i];
            }
        }
        result
    }

    fn prefix(&self, mut end: usize) -> f64 {
        let mut sum = 0.;
        while end > 0 {
            sum += self.tree[end];
            end &= end - 1;
        }
        sum
    }

    fn total(&self) -> f64 {
        self.prefix(self.values.len())
    }

    fn set(&mut self, index: usize, height: f64) {
        let delta = height - self.values[index];
        self.values[index] = height;
        let mut i = index + 1;
        while i < self.tree.len() {
            self.tree[i] += delta;
            i += i.isolate_lowest_one();
        }
    }

    /// Row containing y; clamp the end to the last row, including empty lists.
    fn row_at(&self, y: f64) -> usize {
        let mut index = 0;
        let mut sum = 0.;
        let mut bit = self.tree.len().next_power_of_two() / 2;
        while bit > 0 {
            let next = index + bit;
            if next < self.tree.len() && sum + self.tree[next] <= y {
                index = next;
                sum += self.tree[next];
            }
            bit /= 2;
        }
        index.min(self.values.len().saturating_sub(1))
    }
}

#[derive(Default)]
struct Cache {
    row: Option<MessageRow>,
    day_label: Option<slint::SharedString>,
    prefix: Option<f64>,
    body_height: f32,
    measured_at: u64,
}

struct PhotoImage {
    pixels: Option<Arc<crate::photos::Pixels>>,
    image: slint::Image,
    state: crate::photos::State,
}

#[derive(Default)]
struct State {
    source: Arc<[domain::Message]>,
    cache: Vec<Cache>,
    heights: Heights,
    width: f32,
    font: slint::SharedString,
    viewport: f64,
    offset: f64,
    generation: u64,
    bottom: bool,
    group: bool,
    transcript: Arc<str>,
    images: HashMap<i32, (Arc<crate::photos::Pixels>, slint::Image)>,
    today: chrono::NaiveDate,
    photos: HashMap<i32, PhotoImage>,
    viewer: Option<(i32, Arc<crate::photos::Pixels>, slint::Image)>,
}

impl State {
    fn anchor(&self) -> (usize, f64) {
        let index = self.heights.row_at(self.offset);
        (index, self.offset - self.heights.prefix(index))
    }

    fn restore(&mut self, anchor: (usize, f64)) {
        let maximum = (self.heights.total() - self.viewport).max(0.);
        self.offset = if self.bottom || self.source.is_empty() {
            maximum
        } else {
            let index = anchor.0.min(self.source.len() - 1);
            // Stay inside the anchor if a reflow/removal replaces a tall row
            // with a shorter one; its bottom boundary belongs to the next row.
            (self.heights.prefix(index) + anchor.1.min((self.heights.values[index] - 1.).max(0.)))
                .clamp(0., maximum)
        };
    }

    fn row(&mut self, index: usize) -> Option<MessageRow> {
        let message = self.source.get(index)?;
        let mut row = self.cache[index]
            .row
            .get_or_insert_with(|| {
                let previous = index.checked_sub(1).map(|i| &self.source[i]);
                super::message_row(message, previous)
            })
            .clone();
        if row.has_thumbnail {
            row.thumbnail = self
                .images
                .get(&message.id)
                .map(|(_, image)| image.clone())
                .unwrap_or_default();
        }
        if row.has_photo
            && let Some(photo) = self.photos.get(&message.id)
        {
            row.photo_image = photo.image.clone();
            row.photo_state = photo.state as i32;
            row.photo_status = photo.state.label().into();
        }
        Some(row)
    }

    fn displayed(&mut self, index: usize) -> TimelineRow {
        let day_label = self.cache[index]
            .day_label
            .get_or_insert_with(|| {
                let message = &self.source[index];
                if starts_day(&self.source, index) {
                    crate::message_time::day_label(message.date.expect("dated row"), self.today)
                        .into()
                } else {
                    Default::default()
                }
            })
            .clone();
        TimelineRow {
            message: self.row(index).expect("visible row is in range"),
            top: self.heights.prefix(index) as f32,
            first: index == 0,
            last: index + 1 == self.source.len(),
            day_label,
            body_height: self.cache[index].body_height,
        }
    }

    fn refresh_date(&mut self, today: chrono::NaiveDate) -> bool {
        if self.today == today {
            return false;
        }
        self.today = today;
        for cache in &mut self.cache {
            cache.day_label = None;
        }
        // Single-line chips retain their height when their wording changes.
        // No row remeasurement, model reset or scroll-anchor change is needed.
        true
    }

    fn update(&mut self, source: Arc<[domain::Message]>, reset: bool, group: bool) -> bool {
        if !reset && Arc::ptr_eq(&self.source, &source) && self.group == group {
            return false;
        }
        let old_anchor = self.anchor();
        let anchor_id = self.source.get(old_anchor.0).map(|m| m.id);
        let old_indices: HashMap<_, _> = self
            .source
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id, i))
            .collect();
        let mut old_cache = std::mem::take(&mut self.cache);
        let mut values = Vec::with_capacity(source.len());
        let mut changed = reset || self.source.len() != source.len() || self.group != group;
        let mut anchor_index = None;
        for (i, message) in source.iter().enumerate() {
            if Some(message.id) == anchor_id {
                anchor_index = Some(i);
            }
            let old = old_indices.get(&message.id).copied();
            let same = old.is_some_and(|j| {
                self.source[j] == *message
                    && starts_run(&self.source, j) == starts_run(&source, i)
                    && starts_day(&self.source, j) == starts_day(&source, i)
                    && (j == 0) == (i == 0)
                    && (j + 1 == self.source.len()) == (i + 1 == source.len())
            });
            changed |= !same || old != Some(i);
            if same && !reset && self.group == group {
                let j = old.unwrap();
                self.cache.push(std::mem::take(&mut old_cache[j]));
                values.push(self.heights.values[j]);
            } else {
                self.cache.push(Cache {
                    prefix: old.and_then(|j| old_cache[j].prefix),
                    ..Cache::default()
                });
                // Reuse a previous height as an estimate after an edit. New
                // text rows start at a modest estimate; visible rows are exact.
                values.push(old.map_or(72., |j| self.heights.values[j]));
            }
        }
        // If expiration removed the anchor, keep the next surviving message.
        let anchor_index = anchor_index.unwrap_or_else(|| {
            let new_indices: HashMap<_, _> =
                source.iter().enumerate().map(|(i, m)| (m.id, i)).collect();
            self.source
                .iter()
                .skip(old_anchor.0)
                .find_map(|m| new_indices.get(&m.id).copied())
                .unwrap_or(old_anchor.0.min(source.len().saturating_sub(1)))
        });
        self.source = source;
        self.heights = Heights::new(values);
        self.group = group;
        self.bottom |= reset;
        self.restore((anchor_index, if reset { 0. } else { old_anchor.1 }));
        changed
    }
}

fn starts_run(messages: &[domain::Message], index: usize) -> bool {
    messages[index].starts_run(index.checked_sub(1).map(|i| &messages[i]))
}

fn starts_day(messages: &[domain::Message], index: usize) -> bool {
    messages[index].starts_day(index.checked_sub(1).map(|i| &messages[i]))
}

pub(super) struct Timeline {
    state: RefCell<State>,
    history: RefCell<History>,
    visible: Rc<VecModel<TimelineRow>>,
    notify: ModelNotify,
    laying_out: Cell<bool>,
    _calendar_timer: slint::Timer,
}

#[derive(Default)]
struct History {
    chat: i32,
    sequence: i32,
    pending: Option<i32>,
    retry_blocked: bool,
}

impl Timeline {
    fn install(ui: &AppWindow) -> Rc<Self> {
        let model = Rc::new(Self {
            state: RefCell::new(State {
                bottom: true,
                generation: 1,
                today: chrono::Local::now().date_naive(),
                ..State::default()
            }),
            history: RefCell::new(History::default()),
            visible: Rc::new(VecModel::default()),
            notify: ModelNotify::default(),
            laying_out: Cell::new(false),
            _calendar_timer: slint::Timer::default(),
        });
        let weak = ui.as_weak();
        let weak_model = Rc::downgrade(&model);
        // One minute check for the open timeline, not one timer per message.
        // Also catches wake-from-sleep and manual clock changes. Most checks
        // leave every Slint property and cached height untouched.
        model._calendar_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(60),
            move || {
                if let (Some(ui), Some(model)) = (weak.upgrade(), weak_model.upgrade()) {
                    let changed = model
                        .state
                        .borrow_mut()
                        .refresh_date(chrono::Local::now().date_naive());
                    if changed {
                        model.layout(&ui, true);
                    }
                }
            },
        );
        ui.set_messages(ModelRc::from(model.clone()));
        ui.set_timeline_rows(ModelRc::from(model.visible.clone()));
        let weak_model = Rc::downgrade(&model);
        ui.on_prepare_transcript(move || {
            weak_model.upgrade().map_or_else(Default::default, |model| {
                model.state.borrow().transcript.as_ref().into()
            })
        });
        let weak = ui.as_weak();
        ui.on_layout_timeline(move || {
            if let Some(ui) = weak.upgrade() {
                layout(&ui);
            }
        });
        let weak = ui.as_weak();
        ui.on_follow_timeline_bottom(move || {
            if let Some(ui) = weak.upgrade() {
                with(&ui, |model| {
                    model.state.borrow_mut().bottom = true;
                    model.layout(&ui, false);
                });
            }
        });
        model
    }

    fn read_scroll(&self, ui: &AppWindow) {
        let mut state = self.state.borrow_mut();
        // A resize/composer expansion may clamp the toolkit's old scroll
        // extent. It is not a reader scroll and must not replace the anchor.
        if (ui.get_timeline_width() - state.width).abs() > 0.1
            || ui.get_text_layout_font() != state.font
            || (f64::from(ui.get_timeline_visible()) - state.viewport).abs() > 0.1
        {
            return;
        }
        let offset = f64::from(-ui.get_timeline_y()).max(0.);
        if (offset - state.offset).abs() > 0.5 {
            state.offset = offset;
            state.bottom = state.heights.total() - offset - state.viewport <= 2.;
        }
    }

    fn layout(&self, ui: &AppWindow, read_scroll: bool) {
        if self.laying_out.replace(true) {
            return;
        }
        let started = SAMPLES.with_borrow(|s| s.as_ref().map(|_| Instant::now()));
        let mut measured_rows = 0;
        if read_scroll {
            self.read_scroll(ui);
        }
        let mut state = self.state.borrow_mut();
        let width = ui.get_timeline_width();
        let viewport = f64::from(ui.get_timeline_visible());
        if width <= 0. || viewport <= 0. {
            self.laying_out.set(false);
            return;
        }
        let anchor = state.anchor();
        let old_prefix = state.cache.get(anchor.0).and_then(|c| c.prefix);
        if (width - state.width).abs() > 0.1 || ui.get_text_layout_font() != state.font {
            state.width = width;
            state.font = ui.get_text_layout_font();
            state.generation += 1;
        }
        state.viewport = viewport;
        state.restore(anchor);
        // A whole extra screen on each side multiplies retained toolkit layout
        // work on resize. A small look-ahead covers typical wheel steps while
        // leaving the bulk of offscreen rows as cached data only.
        let overscan = (viewport * 0.25).min(128.);
        // Measuring may move the estimated window. Continue until its rows
        // have exact heights; each row can be measured only once per generation.
        loop {
            if state.source.is_empty() {
                break;
            }
            let start = state.heights.row_at((state.offset - overscan).max(0.));
            let end = (state.heights.row_at(state.offset + viewport + overscan) + 1)
                .min(state.source.len());
            let mut measured = false;
            for i in start..end {
                if state.cache[i].measured_at == state.generation {
                    continue;
                }
                ui.set_timeline_probe(state.displayed(i));
                let height = f64::from(ui.get_timeline_measured_height());
                state.cache[i].body_height = ui.get_timeline_measured_body_height();
                state.cache[i].prefix = Some(f64::from(ui.get_timeline_measured_prefix()));
                // A malformed/temporarily unavailable measurement must not
                // produce a zero-height infinite virtualization loop.
                if height.is_finite() && height >= 1. {
                    state.heights.set(i, height);
                }
                state.cache[i].measured_at = state.generation;
                measured_rows += 1;
                measured = true;
            }
            // Loading history can remove a chip above the first visible
            // bubble. Preserve the reader's position inside that bubble,
            // rather than the old distance from the now-removed decoration.
            let within = match (old_prefix, state.cache[anchor.0].prefix) {
                (Some(old), Some(new)) if anchor.1 >= old => anchor.1 + new - old,
                (Some(_), Some(new)) => anchor.1.min(new),
                _ => anchor.1,
            };
            state.restore((anchor.0, within));
            if !measured {
                break;
            }
        }
        let rows = if state.source.is_empty() {
            Vec::new()
        } else {
            let start = state.heights.row_at((state.offset - overscan).max(0.));
            let end = (state.heights.row_at(state.offset + viewport + overscan) + 1)
                .min(state.source.len());
            (start..end).map(|i| state.displayed(i)).collect()
        };
        // A read boundary comes from the laid-out source, not the archive's
        // newest row (which might have arrived since this snapshot).
        let read_tail = if state.heights.total() - state.offset - viewport <= 2. {
            state.source.last().map_or(0, |m| m.id)
        } else {
            0
        };
        let total = state.heights.total() as f32;
        let y = -state.offset as f32;
        drop(state);
        reconcile_visible(&self.visible, rows);
        ui.set_timeline_height(total);
        ui.set_timeline_y(y);
        ui.set_read_tail(read_tail);
        self.laying_out.set(false);
        ui.invoke_request_message_images();
        ui.invoke_request_photos();
        if let Some(started) = started {
            SAMPLES.with_borrow_mut(|samples| {
                if let Some(samples) = samples {
                    samples
                        .milliseconds
                        .push(started.elapsed().as_secs_f64() * 1000.);
                    samples.measured_rows += measured_rows;
                }
            });
        }
        self.request_history(ui);
    }

    fn request_history(&self, ui: &AppWindow) {
        let active = ui.get_history_active();
        let older = ui.get_older_count();
        let chat = ui.get_selected_id();
        let state = self.state.borrow();
        let near_top = state.offset < 80. || state.heights.total() <= state.viewport + 1.;
        let mut history = self.history.borrow_mut();
        if !near_top {
            history.retry_blocked = false;
        }
        if !near_top
            || (state.source.is_empty() && !ui.get_phone_history_available())
            || (older == 0 && !ui.get_phone_history_available())
            || !active
            || history.chat != chat
            || history.pending.is_some()
            || history.retry_blocked
        {
            return;
        }
        history.sequence = history.sequence.wrapping_add(1).max(1);
        let request = history.sequence;
        history.pending = Some(request);
        let chat = history.chat;
        drop(history);
        drop(state);
        // Only enqueue work. Loading/decryption stays on the archive worker.
        if !ui.invoke_load_older(chat, request) {
            self.finish_history(
                ui,
                domain::HistoryReply {
                    chat,
                    request,
                    success: false,
                },
            );
        }
    }

    fn finish_history(&self, ui: &AppWindow, reply: domain::HistoryReply) {
        let mut history = self.history.borrow_mut();
        if history.chat != reply.chat || history.pending != Some(reply.request) {
            return;
        }
        history.pending = None;
        // An error retries on a fresh approach to the top, not every layout.
        history.retry_blocked = !reply.success;
        drop(history);
        if reply.success {
            self.request_history(ui);
        }
    }

    fn update(&self, ui: &AppWindow, view: &domain::Conversation, reset: bool) -> bool {
        let mut history = self.history.borrow_mut();
        if history.chat != view.chat.id || reset {
            history.chat = view.chat.id;
            history.pending = None;
            history.retry_blocked = false;
        }
        drop(history);
        self.read_scroll(ui);
        let mut state = self.state.borrow_mut();
        let changed = state.update(view.messages.clone(), reset, view.is_group);
        state.transcript = view.transcript.clone();
        drop(state);
        if changed {
            self.notify.reset();
        }
        self.layout(ui, false);
        changed
    }
}

/// Preserve existing widget instances when the virtual window slides. Full
/// replacement is reserved for a seek whose old/new windows do not overlap.
fn reconcile_visible(model: &VecModel<TimelineRow>, rows: Vec<TimelineRow>) {
    if rows.is_empty() {
        if model.row_count() > 0 {
            model.set_vec(rows);
        }
        return;
    }
    let first = rows[0].message.id;
    if let Some(remove) =
        (0..model.row_count()).find(|&i| model.row_data(i).unwrap().message.id == first)
    {
        for _ in 0..remove {
            model.remove(0);
        }
    } else if let Some(old) = model.row_data(0) {
        if let Some(add) = rows.iter().position(|row| row.message.id == old.message.id) {
            for (i, row) in rows[..add].iter().enumerate() {
                model.insert(i, row.clone());
            }
        } else {
            model.set_vec(rows);
            return;
        }
    }
    while model.row_count() > rows.len() {
        model.remove(model.row_count() - 1);
    }
    for (i, row) in rows.into_iter().enumerate() {
        if i >= model.row_count() {
            model.push(row);
        } else if !model
            .row_data(i)
            .as_ref()
            .is_some_and(|old| same_row(old, &row))
        {
            model.set_row_data(i, row);
        }
    }
}

fn same_row(a: &TimelineRow, b: &TimelineRow) -> bool {
    let (am, bm) = (&a.message, &b.message);
    // Slint's empty Image compares unequal even to itself. Avoid invalidating
    // every visible text row when no thumbnail has changed.
    a.top == b.top
        && a.body_height == b.body_height
        && a.first == b.first
        && a.last == b.last
        && a.day_label == b.day_label
        && am.id == bm.id
        && am.system_notice == bm.system_notice
        && am.text == bm.text
        && am.sender == bm.sender
        && am.sender_initials == bm.sender_initials
        && am.time == bm.time
        && am.outgoing == bm.outgoing
        && am.local == bm.local
        && am.first_in_run == bm.first_in_run
        && am.preview_url == bm.preview_url
        && am.preview_title == bm.preview_title
        && am.preview_description == bm.preview_description
        && am.preview_domain == bm.preview_domain
        && am.body_url == bm.body_url
        && am.actions == bm.actions
        && am.has_thumbnail == bm.has_thumbnail
        && am.has_attachment == bm.has_attachment
        && am.has_document == bm.has_document
        && am.document_name == bm.document_name
        && am.document_type == bm.document_type
        && am.document_details == bm.document_details
        && am.has_audio == bm.has_audio
        && am.actionable == bm.actionable
        && am.quote_sender == bm.quote_sender
        && am.quote_text == bm.quote_text
        && am.forwarded == bm.forwarded
        && am.reactions == bm.reactions
        && am.own_reaction == bm.own_reaction
        && am.reaction_status == bm.reaction_status
        && am.audio_waveform == bm.audio_waveform
        && am.audio_duration == bm.audio_duration
        && am.has_photo == bm.has_photo
        && am.photo_width == bm.photo_width
        && am.photo_height == bm.photo_height
        && am.photo_state == bm.photo_state
        && am.photo_status == bm.photo_status
        && same_image(&am.photo_image, &bm.photo_image)
        && ((am.thumbnail.size().width == 0 && bm.thumbnail.size().width == 0)
            || am.thumbnail == bm.thumbnail)
}

fn same_image(a: &slint::Image, b: &slint::Image) -> bool {
    (a.size().width == 0 && b.size().width == 0) || a == b
}

fn photo_image(p: &crate::photos::Pixels) -> slint::Image {
    slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
        &p.rgba, p.width, p.height,
    ))
}

pub(crate) fn has_photo(ui: &AppWindow, id: i32) -> bool {
    let mut found = false;
    with(ui, |model| {
        found = model
            .state
            .borrow()
            .source
            .iter()
            .any(|m| m.id == id && m.content.photo.is_some())
    });
    found
}

fn viewer(ui: &AppWindow, state: &mut State) {
    let id = ui.get_photo_viewer_id();
    if state.viewer.as_ref().is_some_and(|v| v.0 != id) {
        state.viewer = None;
    }
    let inline = state.photos.get(&id);
    let image = state
        .viewer
        .as_ref()
        .map(|v| v.2.clone())
        .or_else(|| inline.map(|p| p.image.clone()))
        .unwrap_or_default();
    if !same_image(&ui.get_photo_viewer_image(), &image) {
        ui.set_photo_viewer_image(image);
    }
    ui.set_photo_viewer_ready(state.viewer.is_some());
    let status = inline.map_or(crate::photos::State::Idle, |p| p.state);
    ui.set_photo_viewer_status(
        if !ui.get_demo_mode()
            && ui.get_connection_state() != 5
            && status != crate::photos::State::Ready
        {
            "Connect to download this photo"
        } else if status == crate::photos::State::Ready && state.viewer.is_none() {
            "Opening photo…"
        } else {
            status.label()
        }
        .into(),
    );
}

pub(crate) fn refresh_photo_viewer(ui: &AppWindow) {
    with(ui, |model| viewer(ui, &mut model.state.borrow_mut()));
}

pub(crate) fn photos(ui: &AppWindow, snapshot: &crate::photos::Snapshot) {
    with(ui, |model| {
        let mut state = model.state.borrow_mut();
        let mut previous = std::mem::take(&mut state.photos);
        state.photos = snapshot
            .rows
            .iter()
            .take(crate::photos::MAX_DEMAND + 1)
            .map(|(id, p)| {
                let old = previous.remove(id);
                let same = old.as_ref().is_some_and(|o| match (&o.pixels, &p.image) {
                    (None, None) => true,
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    _ => false,
                });
                let image = if same {
                    old.unwrap().image
                } else {
                    p.image.as_ref().map(|p| photo_image(p)).unwrap_or_default()
                };
                (
                    *id,
                    PhotoImage {
                        pixels: p.image.clone(),
                        image,
                        state: p.state,
                    },
                )
            })
            .collect();
        let id = ui.get_photo_viewer_id();
        match &snapshot.viewer {
            Some((photo_id, pixels)) if id == *photo_id => {
                if !state
                    .viewer
                    .as_ref()
                    .is_some_and(|(old_id, old, _)| old_id == photo_id && Arc::ptr_eq(old, pixels))
                {
                    state.viewer = Some((id, pixels.clone(), photo_image(pixels)));
                }
            }
            _ => {
                state.viewer = None;
            }
        }
        viewer(ui, &mut state);
        for i in 0..model.visible.row_count() {
            let mut row = model.visible.row_data(i).unwrap();
            if !row.message.has_photo {
                continue;
            }
            let photo = state.photos.get(&row.message.id);
            let image = photo.map(|p| p.image.clone()).unwrap_or_default();
            let status = photo.map_or(crate::photos::State::Idle, |p| p.state);
            if !same_image(&image, &row.message.photo_image)
                || row.message.photo_state != status as i32
            {
                row.message.photo_image = image;
                row.message.photo_state = status as i32;
                row.message.photo_status = status.label().into();
                model.visible.set_row_data(i, row);
            }
        }
    });
}

impl Model for Timeline {
    type Data = MessageRow;
    fn row_count(&self) -> usize {
        self.state.borrow().source.len()
    }
    fn row_data(&self, index: usize) -> Option<MessageRow> {
        self.state.borrow_mut().row(index)
    }
    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn with(ui: &AppWindow, action: impl FnOnce(&Timeline)) {
    let model = ui.get_messages();
    if let Some(model) = model.as_any().downcast_ref::<Timeline>() {
        action(model);
    }
}
pub(crate) fn selected_ids(ui: &AppWindow, ids: &std::collections::HashSet<i32>) -> Vec<i32> {
    let mut result = Vec::new();
    with(ui, |model| {
        result = model
            .state
            .borrow()
            .source
            .iter()
            .filter(|m| ids.contains(&m.id) && !m.content.unavailable)
            .map(|m| m.id)
            .collect();
    });
    result
}
pub(crate) fn selected_text(ui: &AppWindow, ids: &std::collections::HashSet<i32>) -> String {
    let mut result = String::new();
    with(ui, |model| {
        result = model
            .state
            .borrow()
            .source
            .iter()
            .filter(|m| ids.contains(&m.id) && !m.content.unavailable)
            .map(|m| m.content.body.as_deref().unwrap_or(&m.text))
            .collect::<Vec<_>>()
            .join("\n\n");
    });
    result
}

pub(crate) fn action(ui: &AppWindow, id: i32, index: i32) -> crate::message_content::Target {
    use crate::message_content::Target;
    let mut target = Target::Unavailable;
    with(ui, |model| {
        let state = model.state.borrow();
        let Some(message) = state.source.iter().find(|m| m.id == id) else {
            return;
        };
        target = match index {
            -6 if !message.content.unavailable => Target::Copy(
                message
                    .content
                    .body
                    .as_deref()
                    .unwrap_or(&message.text)
                    .to_owned(),
            ),
            -1 | -4 => message
                .content
                .preview
                .as_ref()
                .map(|p| {
                    if index == -1 {
                        Target::Url(p.url.clone())
                    } else {
                        Target::Copy(p.url.clone())
                    }
                })
                .unwrap_or_default(),
            -2 | -3 => crate::message_content::web_url(
                message.content.body.as_deref().unwrap_or(&message.text),
            )
            .map(|url| {
                if index == -2 {
                    Target::Url(url)
                } else {
                    Target::Copy(url)
                }
            })
            .unwrap_or_default(),
            n if n >= 0 => message
                .content
                .actions
                .get((if n >= 100 { n - 100 } else { n }) as usize)
                .map(|a| match &a.target {
                    Target::Url(url) if n >= 100 => Target::Copy(url.clone()),
                    target => target.clone(),
                })
                .unwrap_or_default(),
            _ => Target::Unavailable,
        };
    });
    target
}

pub(crate) fn images(ui: &AppWindow, snapshot: &crate::message_thumbnails::Snapshot) {
    with(ui, |model| {
        let mut state = model.state.borrow_mut();
        let mut previous = std::mem::take(&mut state.images);
        state.images = snapshot
            .iter()
            .take(crate::message_thumbnails::MAX_DEMAND)
            .map(|(id, photo)| {
                let image = previous
                    .remove(id)
                    .filter(|(old, _)| Arc::ptr_eq(old, photo))
                    .map(|(_, image)| image)
                    .unwrap_or_else(|| {
                        slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                            photo.rgba.as_slice(),
                            photo.width,
                            photo.height,
                        ))
                    });
                (*id, (photo.clone(), image))
            })
            .collect();
        // Image dimensions are reserved in the card; loading never reflows text.
        for i in 0..model.visible.row_count() {
            let mut row = model.visible.row_data(i).unwrap();
            let image = state
                .images
                .get(&row.message.id)
                .map(|(_, image)| image.clone())
                .unwrap_or_default();
            if row.message.has_thumbnail
                && !(image.size().width == 0 && row.message.thumbnail.size().width == 0)
                && row.message.thumbnail != image
            {
                row.message.thumbnail = image;
                model.visible.set_row_data(i, row);
            }
        }
    });
}

pub(super) fn layout(ui: &AppWindow) {
    with(ui, |model| model.layout(ui, true));
}

pub(crate) fn history_finished(ui: &AppWindow, reply: domain::HistoryReply) {
    with(ui, |model| model.finish_history(ui, reply));
}

pub(super) fn update(ui: &AppWindow, view: &domain::Conversation, reset: bool) -> bool {
    let model = ui.get_messages();
    if let Some(model) = model.as_any().downcast_ref::<Timeline>() {
        model.update(ui, view, reset)
    } else {
        Timeline::install(ui).update(ui, view, true)
    }
}

pub(super) fn append(ui: &AppWindow, message: domain::Message, transcript: Arc<str>) {
    with(ui, |model| {
        let mut state = model.state.borrow_mut();
        let mut messages = state.source.to_vec();
        messages.push(message);
        let group = state.group;
        state.update(messages.into(), false, group);
        state.transcript = transcript.clone();
        state.bottom = true;
        drop(state);
        model.notify.reset();
        model.layout(ui, false);
    });
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn height_index_matches_linear_positions_after_edits() {
        let mut values: Vec<_> = (0..10_000).map(|i| 30. + (i % 113) as f64).collect();
        let mut index = Heights::new(values.clone());
        for i in (0..10_000).step_by(19) {
            values[i] = 200. + (i % 37) as f64;
            index.set(i, values[i]);
        }
        let mut top = 0.;
        for (i, height) in values.into_iter().enumerate() {
            assert_eq!(index.prefix(i), top);
            assert_eq!(index.row_at(top), i);
            assert_eq!(index.row_at(top + height - 0.01), i);
            top += height;
        }
        assert_eq!(index.total(), top);
        assert_eq!(index.row_at(top + 100.), 9999);
        assert_eq!(Heights::default().row_at(0.), 0);
    }

    #[test]
    fn anchors_follow_message_identity_through_prepend_edit_and_removal() {
        let mut demo = crate::fixtures::demo();
        demo.select(6);
        let mut view = demo.conversation();
        let original = view.messages.clone();
        let mut state = State {
            viewport: 300.,
            generation: 1,
            ..State::default()
        };
        state.update(original.clone(), true, false);
        state.bottom = false;
        state.offset = state.heights.prefix(30) + 17.;
        let id = original[30].id;
        demo.load_older(6);
        view = demo.conversation();
        state.update(view.messages.clone(), false, false);
        let (row, within) = state.anchor();
        assert_eq!(state.source[row].id, id);
        assert_eq!(within, 17.);
        let mut edited = view.messages.to_vec();
        edited[0].text = "edited above the viewport\n".repeat(100);
        state.update(edited.clone().into(), false, false);
        let anchor = state.anchor();
        state.heights.set(0, 5000.);
        state.restore(anchor);
        assert_eq!(state.source[state.anchor().0].id, id);
        assert_eq!(state.anchor().1, 17.);
        let next = edited[row + 1].id;
        edited.remove(row);
        state.update(edited.into(), false, false);
        assert_eq!(state.source[state.anchor().0].id, next);
        let anchor = state.anchor();
        state.heights.set(anchor.0, 2.);
        state.restore((anchor.0, 1000.));
        assert_eq!(state.source[state.anchor().0].id, next);
        assert_eq!(state.anchor().1, 1.);
        state.update(Arc::from([]), false, false);
        assert_eq!(state.offset, 0.);
        assert_eq!(state.heights.total(), 0.);
    }

    #[test]
    fn date_boundaries_invalidate_neighbors_but_rollover_keeps_heights() {
        let mut demo = crate::fixtures::demo();
        demo.select(8);
        let mut messages = demo.conversation().messages.to_vec();
        let today = messages[2].date.unwrap();
        let mut state = State {
            today,
            generation: 1,
            ..State::default()
        };
        state.update(messages.clone().into(), true, false);
        for i in 0..messages.len() {
            state.displayed(i);
            state.cache[i].measured_at = 1;
        }
        assert_eq!(state.displayed(1).day_label, "Yesterday");
        assert_eq!(state.displayed(2).day_label, "Today");
        assert!(!state.displayed(3).message.first_in_run);
        assert!(state.displayed(4).message.first_in_run);
        assert!(state.displayed(4).day_label.is_empty());
        assert!(!state.refresh_date(today));
        assert!(state.refresh_date(today + chrono::Days::new(1)));
        assert_eq!(state.displayed(2).day_label, "Yesterday");
        assert!(state.cache.iter().all(|c| c.measured_at == 1));
        // Date changes can remove a chip while the sender still starts a run.
        messages[1].sender = "Another sender".into();
        messages[1].sender_id = "demo:another".into();
        messages[1].date = messages[2].date;
        state.update(messages.clone().into(), false, false);
        assert_eq!(state.cache[2].measured_at, 0);
        assert!(state.displayed(2).day_label.is_empty());
        assert!(state.displayed(2).message.first_in_run);
        // Deleting the first message of a day moves its chip to the next one.
        messages.remove(1);
        state.update(messages.clone().into(), false, false);
        assert_eq!(state.displayed(1).day_label, "Yesterday");
        // A paged view starts with a chip even in the middle of a day. Loading
        // earlier messages on that day removes the duplicate boundary.
        state.update(messages[2..].to_vec().into(), true, false);
        assert_eq!(state.displayed(0).day_label, "Yesterday");
        state.update(messages.clone().into(), false, false);
        assert!(state.displayed(2).day_label.is_empty());
        assert_eq!(state.cache[2].measured_at, 0);
        // Unknown timestamps must never acquire a fabricated Today chip.
        messages[1].date = None;
        state.update(messages.into(), false, false);
        assert!(state.displayed(1).day_label.is_empty());
        assert_eq!(state.displayed(2).day_label, "Yesterday");
    }

    fn check_date_layout(ui: &AppWindow) {
        use i_slint_backend_testing::ElementHandle;
        let mut demo = crate::fixtures::demo();
        demo.select(8);
        let mut view = demo.conversation();
        // Short Unicode bodies leave all five rows visible even in a narrow
        // viewport; richer cards are exercised by the 10,000-row checks below.
        for message in Arc::make_mut(&mut view.messages) {
            message.content = Default::default();
            message.text = "A short note · 你好 👋".into();
        }
        ui.set_demo_mode(true);
        ui.set_show_conversation(true);
        for dark in [false, true] {
            ui.global::<crate::Theme>().set_dark(dark);
            for width in [520., 620., 1200., 1920.] {
                ui.window().set_size(slint::LogicalSize::new(width, 900.));
                super::super::conversation(ui, &view, true);
                layout(ui);
                let rows: Vec<_> = ui.get_timeline_rows().iter().collect();
                assert_eq!(rows.len(), 5);
                assert_eq!(rows.iter().filter(|r| !r.day_label.is_empty()).count(), 3);
                assert_eq!(rows[1].day_label, "Yesterday");
                assert_eq!(rows[2].day_label, "Today");
                let mut items: Vec<_> =
                    ElementHandle::find_by_element_type_name(ui, "TimelineItem")
                        .filter(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
                        .collect();
                items.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
                assert_eq!(items.len(), 5);
                let mut previous_bottom: Option<f32> = None;
                for (i, item) in items.iter().enumerate() {
                    let bubble = item
                        .query_descendants()
                        .match_id("MessageBubble::bubble")
                        .find_first()
                        .unwrap();
                    let chip = item
                        .query_descendants()
                        .match_id("TimelineItem::date")
                        .find_first();
                    let top = bubble.absolute_position().y;
                    if i < 3 {
                        let chip = chip.expect("day boundary has one chip");
                        let p = chip.absolute_position();
                        let size = chip.size();
                        assert!(
                            (p.x + size.width / 2.
                                - item.absolute_position().x
                                - item.size().width / 2.)
                                .abs()
                                < 0.2
                        );
                        assert!(
                            (p.y - item.absolute_position().y - if i == 0 { 18. } else { 8. })
                                .abs()
                                < 0.2
                        );
                        assert!((top - p.y - size.height - 7.).abs() < 0.2);
                        assert!(size.width <= item.size().width - 36. + 0.2);
                    } else {
                        assert!(chip.is_none(), "same-day messages allocate no chip");
                        let gap = top - previous_bottom.unwrap();
                        assert!((gap - if i == 3 { 3. } else { 8. }).abs() < 0.2);
                    }
                    previous_bottom = Some(top + bubble.size().height);
                    if i > 0 {
                        assert!(
                            (items[i - 1].absolute_position().y + items[i - 1].size().height
                                - item.absolute_position().y)
                                .abs()
                                < 0.2
                        );
                    }
                }
                // A rollover only changes chip text. It cannot trigger a
                // remeasure or move the viewport, even when a weekday becomes
                // a longer full date.
                take_samples();
                with(ui, |model| {
                    let before = model.state.borrow().heights.values.clone();
                    let offset = model.state.borrow().offset;
                    let next = view.messages[2].date.unwrap() + chrono::Days::new(7);
                    assert!(model.state.borrow_mut().refresh_date(next));
                    model.layout(ui, false);
                    assert_eq!(model.state.borrow().heights.values, before);
                    assert_eq!(model.state.borrow().offset, offset);
                    assert_eq!(take_samples().measured_rows, 0);
                    model
                        .state
                        .borrow_mut()
                        .refresh_date(chrono::Local::now().date_naive());
                });
            }
        }
        // Keep text stationary when loading earlier messages removes the
        // date chip from the first loaded (currently visible) message.
        let today = chrono::Local::now().date_naive();
        for message in Arc::make_mut(&mut view.messages) {
            message.date = Some(today);
        }
        Arc::make_mut(&mut view.messages)[0].text =
            "Synthetic line inside a long message.\n".repeat(80);
        ui.window().set_size(slint::LogicalSize::new(800., 600.));
        super::super::conversation(ui, &view, true);
        ui.set_timeline_y(0.);
        layout(ui);
        with(ui, |model| {
            let prefix = model.state.borrow().cache[0].prefix.unwrap();
            ui.set_timeline_y(-(prefix as f32 + 40.));
        });
        layout(ui);
        let anchor_id = view.messages[0].id;
        let mut messages = view.messages.to_vec();
        let mut earlier = messages[0].clone();
        earlier.id = 42_000;
        earlier.text = "An earlier message on the same day.".into();
        messages.insert(0, earlier);
        view.messages = messages.into();
        super::super::conversation(ui, &view, false);
        with(ui, |model| {
            let state = model.state.borrow();
            let (index, within) = state.anchor();
            assert_eq!(state.source[index].id, anchor_id);
            assert!(
                (within - state.cache[index].prefix.unwrap() - 40.).abs() < 0.2,
                "history prepend moved text within the visible bubble"
            );
        });
    }

    /// Called from the existing single-threaded toolkit integration test.
    pub(crate) fn check_native_layout(ui: &AppWindow) {
        check_date_layout(ui);
        use i_slint_backend_testing::ElementHandle;
        let mut demo = crate::fixtures::demo();
        demo.select(6);
        demo.chats[5].visible_count = 10_000;
        let mut view = demo.conversation();
        ui.set_demo_mode(true);
        ui.set_show_conversation(true);
        ui.window().set_size(slint::LogicalSize::new(1200., 800.));
        super::super::conversation(ui, &view, true);
        layout(ui);
        assert_eq!(ui.get_messages().row_count(), 10_000);
        assert!(
            ui.get_timeline_rows().row_count() < 100,
            "only nearby rows become components"
        );
        with(ui, |model| {
            assert!(
                model
                    .state
                    .borrow()
                    .cache
                    .iter()
                    .filter(|c| c.row.is_some())
                    .count()
                    < 100
            );
            assert!(model.state.borrow().bottom);
        });
        // Read from the middle; preserve a partial first row, not just its ID.
        ui.set_timeline_y(-350_017.);
        layout(ui);
        let anchor = std::cell::Cell::new((0, 0.));
        with(ui, |model| {
            let state = model.state.borrow();
            let (i, offset) = state.anchor();
            anchor.set((state.source[i].id, offset));
            assert!(!state.bottom);
        });
        for (width, height) in [(620., 480.), (1920., 1080.), (800., 600.), (1200., 800.)] {
            ui.window().set_size(slint::LogicalSize::new(width, height));
            layout(ui);
            with(ui, |model| {
                let state = model.state.borrow();
                let (i, within) = state.anchor();
                assert_eq!(
                    state.source[i].id,
                    anchor.get().0,
                    "resize changed the read message"
                );
                assert!(
                    (within - anchor.get().1).abs() < 0.1,
                    "resize shifted the read position"
                );
            });
            let mut elements: Vec<_> = ElementHandle::find_by_element_type_name(ui, "TimelineItem")
                .filter(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
                .collect();
            elements.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
            assert!(!elements.is_empty());
            for pair in elements.windows(2) {
                let end = pair[0].absolute_position().y + pair[0].size().height;
                assert!(
                    (end - pair[1].absolute_position().y).abs() < 0.2,
                    "measured/drawn row heights disagree"
                );
            }
        }
        // Unrelated snapshots do not materialize or invalidate timeline rows.
        let before = ui.get_timeline_rows().iter().collect::<Vec<_>>();
        assert!(!super::super::conversation(ui, &view, false));
        let after = ui.get_timeline_rows();
        assert_eq!(before.len(), after.row_count());
        assert!(
            before
                .iter()
                .zip(after.iter())
                .all(|(a, b)| same_row(a, &b))
        );
        assert_eq!(
            ui.invoke_prepare_transcript().as_str(),
            view.transcript.as_ref()
        );
        // Changed text above the viewport keeps the reader on the same message.
        Arc::make_mut(&mut view.messages)[0].text = "Long edited text\n".repeat(100);
        super::super::conversation(ui, &view, false);
        with(ui, |model| {
            let state = model.state.borrow();
            assert_eq!(state.source[state.anchor().0].id, anchor.get().0);
        });
        ui.invoke_scroll_to_bottom();
        assert!(
            (ui.get_timeline_height() + ui.get_timeline_y() - ui.get_timeline_visible()).abs()
                < 0.2
        );
        // Appending while at the end remains at the end synchronously.
        let mut appended = view.messages.to_vec();
        let mut message = appended.last().unwrap().clone();
        message.id += 1;
        message.text = "Synthetic appended message\n".repeat(10);
        appended.push(message);
        view.messages = appended.into();
        super::super::conversation(ui, &view, false);
        assert!(
            (ui.get_timeline_height() + ui.get_timeline_y() - ui.get_timeline_visible()).abs()
                < 0.2
        );
        // Exercise real geometry across history prepends and deletion of the
        // visible row, including changes to the date/sender-run decorations.
        demo.chats[5].visible_count = 100;
        let recent = demo.conversation();
        super::super::conversation(ui, &recent, true);
        ui.set_timeline_y(-1700.);
        layout(ui);
        with(ui, |model| {
            let state = model.state.borrow();
            let (index, within) = state.anchor();
            anchor.set((state.source[index].id, within));
        });
        demo.load_older(6);
        let mut older = demo.conversation();
        super::super::conversation(ui, &older, false);
        with(ui, |model| {
            let state = model.state.borrow();
            let (index, within) = state.anchor();
            assert_eq!(state.source[index].id, anchor.get().0);
            assert!((within - anchor.get().1).abs() < 0.1);
        });
        // Read inside the bubble, past any date chip. Changing fonts can
        // change that chip's height; preserve the position in the message,
        // rather than its old distance from the top of the decoration.
        with(ui, |model| {
            let state = model.state.borrow();
            let index = state.anchor().0;
            let top = state.heights.prefix(index) + state.cache[index].prefix.unwrap();
            ui.set_timeline_y(-(top as f32 + 5.));
        });
        layout(ui);
        // Changing the font invalidates measured rows without losing the reader's anchor.
        for family in ["", "Inter"] {
            ui.global::<crate::Theme>().set_font_family(family.into());
            layout(ui);
            with(ui, |model| {
                let state = model.state.borrow();
                let (index, within) = state.anchor();
                assert_eq!(state.font, family);
                assert_eq!(state.source[index].id, anchor.get().0);
                let inside_bubble = within - state.cache[index].prefix.unwrap();
                assert!(
                    (inside_bubble - 5.).abs() < 0.1,
                    "font {family:?} moved the read position inside the message: {inside_bubble}"
                );
            });
        }
        let mut remaining = older.messages.to_vec();
        let removed = remaining
            .iter()
            .position(|m| m.id == anchor.get().0)
            .unwrap();
        remaining.remove(removed);
        let next = remaining[removed].id;
        older.messages = remaining.into();
        super::super::conversation(ui, &older, false);
        with(ui, |model| {
            let state = model.state.borrow();
            assert_eq!(state.source[state.anchor().0].id, next);
        });
        older.messages = Arc::from([]);
        super::super::conversation(ui, &older, false);
        assert_eq!(ui.get_timeline_rows().row_count(), 0);
        assert_eq!(ui.get_timeline_height(), 0.);
        assert_eq!(ui.get_timeline_y(), 0.);
        // Rich content retains virtualization at the same history size.
        demo.chats[5].visible_count = 10_000;
        crate::fixtures::fill_message_cards(&mut demo.chats[5].messages);
        // Worst case: every message starts a different day. It must still
        // instantiate only nearby chips/cards, even across long histories.
        for (i, message) in demo.chats[5].messages.iter_mut().enumerate() {
            message.date =
                Some(chrono::Local::now().date_naive() - chrono::Days::new((9999 - i) as u64));
        }
        let cards = demo.conversation();
        super::super::conversation(ui, &cards, true);
        for i in 0..20 {
            ui.window().set_size(slint::LogicalSize::new(
                if i % 2 == 0 { 800. } else { 1200. },
                if i % 2 == 0 { 480. } else { 800. },
            ));
            ui.set_timeline_y(-(i as f32 * 5000.));
            layout(ui);
            assert!(ui.get_timeline_rows().row_count() < 30);
            let mut elements: Vec<_> = ElementHandle::find_by_element_type_name(ui, "TimelineItem")
                .filter(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
                .collect();
            elements.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
            assert!(elements.len() < 30);
            for pair in elements.windows(2) {
                assert!(
                    (pair[0].absolute_position().y + pair[0].size().height
                        - pair[1].absolute_position().y)
                        .abs()
                        < 0.2,
                    "rich rows disagree in iteration {i}: {:?}/{:?} then {:?}/{:?}",
                    pair[0].absolute_position(),
                    pair[0].size(),
                    pair[1].absolute_position(),
                    pair[1].size()
                );
            }
        }
    }
}
