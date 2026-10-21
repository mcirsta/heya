//! Explicit synthetic MP4 through the real decoder, bounded handoff and renderer.
use super::*;
use crate::{presentation_video, ui::VideoPlayback, video};
use tokio::sync::mpsc;

struct Clip(Vec<u8>);
#[async_trait::async_trait]
impl video::Source for Clip {
    async fn fetch(&self, _: &[u8]) -> Result<Vec<u8>, video::Error> {
        Ok(self.0.clone())
    }
}

pub fn start(
    ui: &AppWindow,
    runtime: &Runtime,
) -> Result<Benchmark, slint::SetRenderingNotifierError> {
    const PHASES: [&str; 5] = [
        "video-poster",
        "video-playing",
        "video-resize",
        "video-expanded",
        "video-stopped",
    ];
    ui.invoke_select_chat(14);
    ui.set_show_conversation(true);
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let (source, sources) = watch::channel(None);
    let failure = Arc::new(AtomicBool::new(false));
    let failed = failure.clone();
    let loader = runtime.spawn(async move {
        let path = std::env::var_os("HEYA_BENCHMARK_VIDEO_FILE").unwrap();
        match tokio::fs::read(path).await {
            Ok(bytes) if !bytes.is_empty() && bytes.len() <= video::MAX_BYTES => {
                source.send_replace(Some(Arc::new(Clip(bytes)) as Arc<dyn video::Source>));
                std::future::pending::<()>().await;
            }
            _ => {
                failed.store(true, Ordering::Release);
            }
        }
    });
    let (commands, requests) = mpsc::channel(16);
    presentation_video::bind(ui, commands);
    let (updates, snapshots) = watch::channel(video::Status::default());
    let producer = runtime.spawn(video::run(
        watch::channel(None).1,
        sources,
        requests,
        updates,
        true,
        watch::channel(crate::preferences::Preferences::default()).1,
    ));
    let times = Arc::new(std::sync::Mutex::new(Vec::new()));
    let apply_times = times.clone();
    let pump = runtime.spawn(live_presentation::present_latest(
        ui.as_weak(),
        snapshots,
        move |ui, status| {
            let start = Instant::now();
            presentation_video::publish(ui, status);
            apply_times
                .lock()
                .unwrap()
                .push(start.elapsed().as_secs_f64() * 1000.);
        },
    ));
    let samples = Rc::new(RefCell::new(Samples::default()));
    let renders = samples.clone();
    let mut before = None;
    let mut previous_frame = None;
    ui.window()
        .set_rendering_notifier(move |state, _| match state {
            RenderingState::BeforeRendering => {
                let now = Instant::now();
                if let Some(previous) = previous_frame.replace(now) {
                    renders
                        .borrow_mut()
                        .frame_ms
                        .push(now.duration_since(previous).as_secs_f64() * 1000.);
                }
                before = Some(now);
            }
            RenderingState::AfterRendering => {
                if let Some(start) = before.take() {
                    renders
                        .borrow_mut()
                        .render_ms
                        .push(start.elapsed().as_secs_f64() * 1000.);
                }
            }
            _ => {}
        })?;
    let timer = Timer::default();
    let weak = ui.as_weak();
    let failed = failure.clone();
    let started = Instant::now();
    let mut previous_tick = started;
    let mut current = 0;
    let mut last_frame = 0;
    println!(
        "Video benchmark: synthetic H.264/AAC, real finite decoder with silent test sink, 960px/25fps ceiling; 3s poster/play/resize/expanded/stopped phases. Render CPU excludes compositor presentation."
    );
    timer.start(TimerMode::Repeated, TICK, move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let now = Instant::now();
        let mut samples = samples.borrow_mut();
        samples.tick_late_ms.push(
            now.duration_since(previous_tick)
                .saturating_sub(TICK)
                .as_secs_f64()
                * 1000.,
        );
        previous_tick = now;
        samples.apply_ms.append(&mut times.lock().unwrap());
        let next = (started.elapsed().as_secs() / 3) as usize;
        let global = ui.global::<VideoPlayback>();
        if next != current {
            report(PHASES[current], &mut samples);
            let frame = global.get_frame_number();
            println!(
                "Video {}: decoder frames advanced={}, state={}",
                PHASES[current],
                frame.saturating_sub(last_frame).max(0),
                global.get_state()
            );
            if (1..=3).contains(&current) && (frame <= last_frame || global.get_state() != 2) {
                failed.store(true, Ordering::Release);
            }
            if current == 4 && global.get_id() != 0 {
                failed.store(true, Ordering::Release);
            }
            if next >= PHASES.len() {
                let _ = slint::quit_event_loop();
                return;
            }
            last_frame = frame;
            *samples = Samples::default();
            current = next;
            match current {
                1 => ui.invoke_video_control(27003, 0, 0.),
                3 => {
                    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
                    ui.invoke_video_control(27003, 4, 0.);
                }
                4 => ui.invoke_video_control(0, 3, 0.),
                _ => {}
            }
        }
        if current == 2 {
            let step = ((started.elapsed().as_millis() / 32 % 60) as f32 - 30.).abs();
            ui.window().set_size(slint::LogicalSize::new(
                640. + step * 18.,
                500. + step * 10.,
            ));
        }
    });
    Ok(Benchmark {
        failure: Some(failure),
        _timer: timer,
        producer,
        pump,
        photos: vec![loader],
    })
}
