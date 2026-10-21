//! Deliberate, synthetic desktop measurements. Never reads an account or captures pixels.
use crate::{AppWindow, archive::View, domain, fixtures, live_presentation, messaging};
use slint::{ComponentHandle, Model, RenderingState, Timer, TimerMode};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{runtime::Runtime, sync::watch, task::JoinHandle};

#[path = "presentation_album_benchmark.rs"]
mod album_viewer;
#[path = "presentation_archive_benchmark.rs"]
mod archive_pipeline;
#[path = "presentation_photo_benchmark.rs"]
mod photo_archive;
#[path = "presentation_video_benchmark.rs"]
mod video_playback;

const TICK: Duration = Duration::from_millis(16);
const PHASE_SECONDS: u64 = 3;
const PHASES: [&str; 9] = [
    "idle",
    "unchanged-updates",
    "changing-updates",
    "resize",
    "maximize-restore",
    "scroll-100",
    "scroll-10000",
    "resize-10000",
    "composer-10000",
];

#[derive(Default)]
struct Samples {
    render_ms: Vec<f64>,
    frame_ms: Vec<f64>,
    tick_late_ms: Vec<f64>,
    apply_ms: Vec<f64>,
}

fn percentile(values: &mut [f64], fraction: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values
        .get(((values.len().saturating_sub(1)) as f64 * fraction).round() as usize)
        .copied()
        .unwrap_or(0.)
}

fn report(name: &str, samples: &mut Samples) {
    let mut layout = super::timeline::take_samples();
    println!(
        "UI benchmark {name}: frames={}, render CPU p50={:.2}ms p95={:.2}ms; frame interval p95={:.2}ms; timer lateness p95={:.2}ms max={:.2}ms; snapshot apply count={} p95={:.2}ms; render CPU max={:.2}ms",
        samples.render_ms.len(),
        percentile(&mut samples.render_ms, 0.5),
        percentile(&mut samples.render_ms, 0.95),
        percentile(&mut samples.frame_ms, 0.95),
        percentile(&mut samples.tick_late_ms, 0.95),
        percentile(&mut samples.tick_late_ms, 1.0),
        samples.apply_ms.len(),
        percentile(&mut samples.apply_ms, 0.95),
        percentile(&mut samples.render_ms, 1.0),
    );
    println!(
        "Timeline {name}: layout calls={}, p95={:.2}ms; measured rows={}",
        layout.milliseconds.len(),
        percentile(&mut layout.milliseconds, 0.95),
        layout.measured_rows
    );
}

pub struct Benchmark {
    failure: Option<Arc<AtomicBool>>,
    _timer: Timer,
    producer: JoinHandle<()>,
    pump: JoinHandle<()>,
    photos: Vec<JoinHandle<()>>,
}

impl Benchmark {
    pub fn failed(&self) -> bool {
        self.failure
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    }
}

impl Drop for Benchmark {
    fn drop(&mut self) {
        self.producer.abort();
        self.pump.abort();
        for worker in &self.photos {
            worker.abort();
        }
    }
}

pub fn start(
    ui: &AppWindow,
    runtime: &Runtime,
) -> Result<Benchmark, slint::SetRenderingNotifierError> {
    if std::env::var_os("HEYA_BENCHMARK_ALBUM").is_some() {
        return album_viewer::start(ui, runtime);
    }
    if std::env::var_os("HEYA_BENCHMARK_VIDEO_FILE").is_some() {
        return video_playback::start(ui, runtime);
    }
    if std::env::var_os("HEYA_BENCHMARK_ARCHIVE").is_some() {
        return archive_pipeline::start(ui, runtime);
    }
    let long_draft = std::env::var_os("HEYA_BENCHMARK_LONG_DRAFT").is_some();
    if long_draft {
        println!("Composer stress: 8–64 KiB synthetic multiline drafts; no account or network");
    }
    let wallpaper = std::env::var_os("HEYA_BENCHMARK_NO_WALLPAPER").is_none();
    ui.global::<crate::Theme>().set_show_wallpaper(wallpaper);
    println!(
        "UI benchmark wallpaper: {}",
        if wallpaper { "on" } else { "off" }
    );
    super::timeline::take_samples();
    let mut state = fixtures::demo();
    let group = std::env::var_os("HEYA_BENCHMARK_GROUP_AVATARS").is_some();
    if group {
        state.chats[5].is_group = true;
        for (i, message) in state.chats[5].messages.iter_mut().enumerate() {
            if !message.outgoing {
                message.sender_id = format!("demo:participant-{}", (i / 4) % 8);
                message.sender = format!("~Sample participant {}", (i / 4) % 8);
            }
        }
        println!(
            "Group avatar benchmark: eight participants, 10,000 messages; cached 128px thumbnails"
        );
    }
    if std::env::var_os("HEYA_BENCHMARK_CHAT_EMOJI").is_some() {
        for (i, message) in state.chats[5].messages.iter_mut().enumerate() {
            message.text = match i % 4 {
                0 => "😟",
                1 => "👩🏽‍💻 🇷🇴 ❤️",
                2 => "Mixed text 😟 · ❤️ · 👩🏽‍💻 · 🇷🇴 · #️⃣",
                _ => "😀😃😄😁",
            }
            .into();
            Arc::make_mut(&mut message.content).reaction_summary = "❤️ 12  👍🏽 2  😟".into();
        }
        println!(
            "Chat emoji benchmark: 10,000 synthetic messages, mixed text, ZWJ/flags and reaction counts"
        );
    }
    let notices = std::env::var_os("HEYA_BENCHMARK_NOTICES").is_some();
    if std::env::var_os("HEYA_BENCHMARK_CALLS").is_some() {
        fixtures::fill_calls(&mut state.chats[5].messages);
        println!("Call benchmark: 10,000 saved call events; no calling/media service");
    }
    if notices {
        for (index, message) in state.chats[5].messages.iter_mut().enumerate() {
            if index % 3 == 1 {
                let notice = crate::message_content::Notice::MessagePinned;
                message.content = Arc::new(crate::message_content::Content {
                    notice: Some(notice),
                    ..Default::default()
                });
                message.text = notice.description().into();
            }
        }
    }
    println!(
        "UI benchmark system notices: {}",
        if notices { "on" } else { "off" }
    );
    let cards = std::env::var_os("HEYA_BENCHMARK_MESSAGE_CARDS").is_some();
    println!(
        "UI benchmark message cards: {}",
        if cards { "on" } else { "off" }
    );
    if cards {
        fixtures::fill_message_cards(&mut state.chats[5].messages);
        // Fixed synthetic pixels stand in for worker-decoded images; exercise
        // the real bounded visible-image handoff and renderer texture creation.
        let image = fixtures::message_image_samples([0])[&0].clone();
        let weak = ui.as_weak();
        ui.on_request_message_images(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let images = ui
                .get_timeline_rows()
                .iter()
                .filter(|r| r.message.has_thumbnail)
                .take(crate::message_thumbnails::MAX_DEMAND)
                .map(|r| (r.message.id, image.clone()))
                .collect();
            crate::presentation_content::publish_images(&ui, &Arc::new(images));
        });
    }
    let audio = std::env::var_os("HEYA_BENCHMARK_AUDIO").is_some();
    if audio {
        fixtures::fill_audio(&mut state.chats[5].messages);
    }
    println!(
        "UI benchmark received audio: {}",
        if audio { "on" } else { "off" }
    );
    let photos = std::env::var_os("HEYA_BENCHMARK_PHOTOS").is_some();
    let wheel = std::env::var_os("HEYA_BENCHMARK_WHEEL").is_some();
    println!(
        "UI benchmark scroll input: {}",
        if wheel {
            "animated wheel"
        } else {
            "immediate pixel steps (legacy comparison)"
        }
    );
    println!(
        "UI benchmark received photos: {}",
        if photos { "on" } else { "off" }
    );
    if photos {
        fixtures::fill_photos(&mut state.chats[5].messages);
    }
    let documents = std::env::var_os("HEYA_BENCHMARK_DOCUMENTS").is_some();
    if documents {
        fixtures::fill_documents(&mut state.chats[5].messages);
    }
    let photo_workers = if documents {
        println!("Document source: temporary encrypted archive, bounded thumbnail worker");
        photo_archive::start(
            ui,
            runtime,
            state.chats[5].messages.clone(),
            photo_archive::Media::Documents,
        )
    } else if photos && std::env::var_os("HEYA_BENCHMARK_PHOTO_ARCHIVE").is_some() {
        println!("Photo source: temporary encrypted archive, per-message worker decoding");
        photo_archive::start(
            ui,
            runtime,
            state.chats[5].messages.clone(),
            photo_archive::Media::Photos,
        )
    } else {
        Vec::new()
    };
    state.select(6); // A page of 100 messages from the 10,000-message timeline.
    let conversation = state.conversation();
    let chats = (0..500)
        .map(|index| domain::ChatSummary {
            id: index + 1,
            title: format!("Synthetic conversation {index}"),
            initials: "SC".into(),
            tint: 0x91d6bc,
            preview: domain::single_line_summary(
                &"Synthetic multiline preview 👩🏽‍💻\nSecond line\n".repeat(100),
            ),
            time: "14 Oct 12:00".into(),
            call_kind: 0,
            unread: index as usize % 5,
            marked_unread: false,
            muted: false,
            archived: false,
        })
        .collect();
    let view = View {
        unread_chats: 0,
        archived_count: 0,
        archived_view: false,
        chats,
        conversation: Some(conversation),
        can_send: false,
        history_failed: false,
        phone_more: false,
    };
    let snapshot = messaging::Snapshot {
        search: Default::default(),
        jump: Default::default(),
        history: None,
        phone: Default::default(),
        chat_busy: false,
        operation: None,
        destinations: Arc::from([]),
        revision: 0,
        view: Some(Arc::new(view.clone())),
        reply: None,
        notice: String::new(),
    };
    live_presentation::publish_messages(ui, &snapshot);
    let avatar_count = if std::env::var_os("HEYA_BENCHMARK_NO_AVATARS").is_some() {
        0
    } else {
        if group { 48 } else { 66 }
    };
    let mut avatar_samples = (*fixtures::avatar_samples(0..avatar_count)).clone();
    if group && avatar_count > 0 {
        for (target, photo) in fixtures::avatar_samples(0..8).iter() {
            let crate::avatars::Target::Chat(id) = target else {
                unreachable!()
            };
            avatar_samples.insert(
                crate::avatars::Target::Participant(format!("demo:participant-{id}")),
                photo.clone(),
            );
        }
    }
    crate::presentation_avatars::publish(ui, &Arc::new(avatar_samples));
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let emoji = std::env::var_os("HEYA_BENCHMARK_EMOJI").is_some();
    let downloads = std::env::var_os("HEYA_BENCHMARK_DOWNLOADS").is_some();
    let attachments = std::env::var_os("HEYA_BENCHMARK_ATTACHMENTS").is_some();
    let voice = std::env::var_os("HEYA_BENCHMARK_VOICE").is_some();
    if voice {
        println!(
            "Voice benchmark: real synthetic-tone capture/Opus encoding and 10 Hz composer progress; no microphone or send"
        );
    }
    let mut attachment_sample = crate::attachments::Snapshot {
        phase: 4,
        title: "Synthetic attachment recipient".into(),
        total: 100 * 1024 * 1024,
        items: (0..10)
            .map(|i| crate::attachments::Item {
                id: format!("synthetic-benchmark-{i}"),
                name: format!("Synthetic image {i}.png"),
                size: 1024 * 1024,
                preview: Some(Arc::new(crate::photos::Pixels {
                    width: 512,
                    height: 384,
                    rgba: vec![150; 512 * 384 * 4],
                })),
            })
            .collect::<Vec<_>>()
            .into(),
        ..Default::default()
    };
    if attachments {
        println!(
            "Attachment review: 10 synthetic images, upload progress, no files/network; nonmodal upload status; background wheel input enabled"
        );
    }
    if downloads {
        println!("Download panel: synthetic progress, no network or filesystem transfer");
    }
    if emoji {
        println!("Emoji picker: opens after idle; category jumps during measured phases");
    }
    let samples = Rc::new(RefCell::new(Samples::default()));
    let render_samples = samples.clone();
    let mut render_start = None;
    let mut previous_render = None;
    ui.window()
        .set_rendering_notifier(move |state, _| match state {
            RenderingState::BeforeRendering => {
                let now = Instant::now();
                if let Some(previous) = previous_render.replace(now) {
                    render_samples
                        .borrow_mut()
                        .frame_ms
                        .push(now.duration_since(previous).as_secs_f64() * 1000.);
                }
                render_start = Some(now);
            }
            RenderingState::AfterRendering => {
                if let Some(start) = render_start.take() {
                    render_samples
                        .borrow_mut()
                        .render_ms
                        .push(start.elapsed().as_secs_f64() * 1000.);
                }
            }
            _ => {}
        })?;
    let phase = Arc::new(AtomicUsize::new(0));
    let stress_snapshot = {
        state.chats[5].visible_count = 10_000;
        messaging::Snapshot {
            search: Default::default(),
            jump: Default::default(),
            operation: None,
            destinations: Arc::from([]),
            view: Some(Arc::new(View {
                conversation: Some(state.conversation()),
                ..view.clone()
            })),
            ..snapshot.clone()
        }
    };
    let producer_phase = phase.clone();
    let (updates, snapshots) = watch::channel(snapshot.clone());
    let producer = runtime.spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut sequence = 0;
        loop {
            interval.tick().await;
            let phase = producer_phase.load(Ordering::Relaxed);
            if phase == 1 {
                updates.send_replace(snapshot.clone());
            } else if phase == 2 {
                sequence += 1;
                let mut view = view.clone();
                Arc::make_mut(&mut view.chats)[0].preview = format!("Synthetic update {sequence}");
                updates.send_replace(messaging::Snapshot {
                    search: Default::default(),
                    jump: Default::default(),
                    operation: None,
                    destinations: Arc::from([]),
                    view: Some(Arc::new(view)),
                    ..snapshot.clone()
                });
            }
        }
    });
    // The pump callback is Send; keep its timing buffer separate from the
    // render notifier's UI-thread-only state.
    let apply_times = Arc::new(std::sync::Mutex::new(Vec::new()));
    let pump_times = apply_times.clone();
    let pump = runtime.spawn(live_presentation::present_latest(
        ui.as_weak(),
        snapshots,
        move |ui, value| {
            let start = Instant::now();
            live_presentation::publish_messages(ui, value);
            pump_times
                .lock()
                .unwrap()
                .push(start.elapsed().as_secs_f64() * 1000.);
        },
    ));
    let timer = Timer::default();
    let weak = ui.as_weak();
    let start = Instant::now();
    let mut previous_tick = start;
    let mut current_phase = 0;
    let mut ticks = 0u64;
    let mut maximized = false;
    println!(
        "UI benchmark: synthetic 500-chat sidebar with {avatar_count} thumbnails / 100 then 10000 loaded messages; 200 snapshots/s during update phases; {}s total. Render CPU excludes compositor presentation. Profile: {}.",
        PHASE_SECONDS * PHASES.len() as u64,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    timer.start(TimerMode::Repeated, TICK, move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let now = Instant::now();
        let elapsed = now.duration_since(previous_tick);
        previous_tick = now;
        let next_phase = (start.elapsed().as_secs() / PHASE_SECONDS) as usize;
        let mut samples = samples.borrow_mut();
        samples
            .tick_late_ms
            .push(elapsed.saturating_sub(TICK).as_secs_f64() * 1000.);
        samples.apply_ms.append(&mut apply_times.lock().unwrap());
        if next_phase != current_phase {
            if emoji && current_phase > 0 {
                assert_eq!(ui.get_message_dialog(), 4, "benchmark picker closed");
            }
            report(PHASES[current_phase], &mut samples);
            if audio {
                println!(
                    "Audio residency: {} instantiated timeline rows; playback state {}",
                    ui.get_timeline_rows().row_count(),
                    ui.global::<crate::ui::AudioPlayback>().get_state()
                );
            }
            if documents {
                let rows = ui.get_timeline_rows();
                let images = rows
                    .iter()
                    .filter(|r| r.message.thumbnail.size().width > 0)
                    .count();
                println!(
                    "Document residency: {} instantiated timeline rows, {images} loaded thumbnails",
                    rows.row_count()
                );
            }
            if photos {
                let rows = ui.get_timeline_rows();
                let images = rows
                    .iter()
                    .filter(|r| r.message.photo_image.size().width > 0)
                    .count();
                println!(
                    "Photo residency: {} instantiated timeline rows, {images} loaded inline images",
                    rows.row_count()
                );
            }
            *samples = Samples::default();
            if next_phase >= PHASES.len() {
                phase.store(PHASES.len(), Ordering::Relaxed);
                let _ = slint::quit_event_loop();
                return;
            }
            current_phase = next_phase;
            phase.store(current_phase, Ordering::Relaxed);
            ticks = 0;
            if voice && current_phase == 1 {
                ui.invoke_voice_control(0);
            }
            if voice && current_phase > 1 {
                assert_eq!(
                    ui.get_voice_phase(),
                    1,
                    "synthetic recording stopped unexpectedly"
                );
            }
            if emoji && current_phase == 1 {
                // Opening before show() races the initial selected-chat change
                // handler. Start after the first frame and verify every phase.
                ui.invoke_open_composer_emoji(380., 740.);
                assert_eq!(ui.get_message_dialog(), 4, "benchmark picker did not open");
            }
            if current_phase == 5 || current_phase == 6 || current_phase == 8 {
                ui.window().set_maximized(false);
                ui.window().set_size(slint::LogicalSize::new(1200., 800.));
            }
            if current_phase == 6 {
                live_presentation::publish_messages(&ui, &stress_snapshot);
                ui.invoke_scroll_to_bottom();
            }
            if audio
                && current_phase >= 5
                && let Some(row) = ui.get_timeline_rows().iter().find(|r| r.message.has_audio)
            {
                ui.invoke_audio_control(0, 3, 0.);
                ui.invoke_audio_control(row.message.id, 0, 0.);
            }
        }
        let path_tick = (start.elapsed().as_millis() % 3000) as u64 / 16;
        if current_phase == 3 || current_phase == 7 {
            let step = (path_tick % 60).abs_diff(30) as f32;
            ui.window().set_size(slint::LogicalSize::new(
                620. + step * 26.,
                480. + step * 14.,
            ));
        } else if current_phase == 4 && ticks.is_multiple_of(30) {
            maximized = !maximized;
            ui.window().set_maximized(maximized);
        } else if current_phase == 5 || current_phase == 6 {
            let dy = if (path_tick / 60).is_multiple_of(2) {
                80.
            } else {
                -80.
            };
            if wheel {
                crate::presentation_scroll::dispatch_wheel(
                    ui.window(),
                    slint::LogicalPosition::new(800., 350.),
                    0.,
                    dy,
                );
            } else {
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                        position: slint::LogicalPosition::new(800., 350.),
                        delta_x: 0.,
                        delta_y: dy,
                    });
            }
        } else if current_phase == 8 {
            ui.set_draft(
                "Synthetic composer line\n"
                    .repeat(((path_tick as usize / 4) % 8 + 1) * if long_draft { 340 } else { 1 })
                    .into(),
            );
        }
        if emoji && ticks.is_multiple_of(15) {
            ui.invoke_jump_emoji_category((ticks / 15 % 9) as i32 + 1);
        }
        if downloads && ticks.is_multiple_of(6) {
            crate::presentation_downloads::publish(
                &ui,
                &crate::downloads::Status {
                    phase: crate::downloads::Phase::Downloading,
                    name: "Synthetic attachment export.pdf".into(),
                    bytes: (ticks % 100) * 1024 * 1024,
                    total: Some(100 * 1024 * 1024),
                    ..Default::default()
                },
            );
        }
        if attachments && ticks.is_multiple_of(6) {
            attachment_sample.bytes = (ticks % 100) * 1024 * 1024;
            crate::presentation_attachments::publish(&ui, &attachment_sample);
        }
        ticks += 1;
    });
    Ok(Benchmark {
        failure: None,
        _timer: timer,
        producer,
        pump,
        photos: photo_workers,
    })
}
