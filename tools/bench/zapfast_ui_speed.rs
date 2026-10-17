//! Build as a binary in a disposable copy of the pinned ZapFast checkout.
//! Synthetic UI only: App::headless supplies a detached/offline backend.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use zapfast::{
    app::{App, Conversation},
    backend::LinkStatus,
    model::{Chat, Content, Delivery, LastMessage, Message},
    paths::AppDirs,
    settings::Settings,
};

const PHASES: [&str; 6] = [
    "warmup",
    "scroll-100",
    "resize-100",
    "scroll-10000",
    "resize-10000",
    "composer-10000",
];

fn text(index: usize) -> String {
    match index % 4 {
        0 => format!("Message {} · A short note.", index + 1),
        1 => format!(
            "Message {} · A longer message to exercise wrapping as the window changes size. A readable conversation should stay comfortable on a small screen too.",
            index + 1
        ),
        2 => format!(
            "Message {}\nFirst line.\nSecond line.\nOne more for good measure.",
            index + 1
        ),
        _ => format!("Message {} · Salut 👋 · مرحبا · こんにちは", index + 1),
    }
}

fn conversation(count: usize) -> Conversation {
    let mut view = Conversation::default();
    view.complete = true;
    view.requested = true;
    view.messages = (10000 - count..10000)
        .map(|index| Message {
            id: index.to_string(),
            chat: "6@s.whatsapp.net".into(),
            sender: if index % 3 == 0 {
                "1@s.whatsapp.net"
            } else {
                "6@s.whatsapp.net"
            }
            .into(),
            sender_name: Some("Timeline lab".into()),
            from_me: index % 3 == 0,
            // Keep every row on one day, matching Heya's single date divider.
            timestamp: 1791936000 + (index % 1440) as i64 * 60,
            history_order: None,
            content: Content::text(text(index)),
            status: Delivery::None,
            delivered_at: None,
            read_at: None,
            quoted: None,
            reactions: vec![],
            edited: false,
            mentions: vec![],
            forwarded: false,
            thumbnail: None,
        })
        .collect();
    view
}

fn percentile(values: &mut [f64], fraction: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values
        .get(((values.len().saturating_sub(1)) as f64 * fraction).round() as usize)
        .copied()
        .unwrap_or(0.)
}

struct Bench {
    app: App,
    start: Instant,
    phase: usize,
    last_frame: Instant,
    last_tick: u64,
    frames: Vec<f64>,
    cpu: Vec<f64>,
    stop: Arc<AtomicBool>,
}

impl Bench {
    fn report(&mut self) {
        println!(
            "ZapFast {}: frames={}; frame interval p95={:.2}ms max={:.2}ms; eframe CPU (UI + rendering) p95={:.2}ms",
            PHASES[self.phase],
            self.frames.len(),
            percentile(&mut self.frames, 0.95),
            percentile(&mut self.frames, 1.),
            percentile(&mut self.cpu, 0.95)
        );
        self.frames.clear();
        self.cpu.clear();
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl eframe::App for Bench {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        let elapsed = self.start.elapsed();
        let phase = elapsed.as_secs() as usize / 3;
        if phase != self.phase {
            self.report();
            if phase >= PHASES.len() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            self.phase = phase;
            if phase == 1 || phase == 3 || phase == 5 {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1200., 800.)));
            }
            if phase == 3 {
                self.app
                    .conversations
                    .get_mut("6@s.whatsapp.net")
                    .unwrap()
                    .messages = conversation(10000).messages;
                self.app.scroll_to_bottom = true;
            }
        }
        let tick = elapsed.as_millis() as u64 / 16;
        if tick == self.last_tick {
            return;
        }
        self.last_tick = tick;
        let local_tick = (elapsed.as_millis() % 3000) as u64 / 16;
        match self.phase {
            1 | 3 => {
                input
                    .events
                    .push(egui::Event::PointerMoved(egui::pos2(800., 350.)));
                input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    delta: egui::vec2(
                        0.,
                        if (local_tick / 60).is_multiple_of(2) {
                            80.
                        } else {
                            -80.
                        },
                    ),
                    modifiers: egui::Modifiers::NONE,
                });
            }
            2 | 4 => {
                let step = (local_tick % 60).abs_diff(30) as f32;
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    620. + step * 26.,
                    480. + step * 14.,
                )));
            }
            5 => {
                self.app.composer =
                    "Synthetic composer line\n".repeat((local_tick as usize / 4) % 8 + 1)
            }
            _ => {}
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let now = Instant::now();
        self.frames
            .push(now.duration_since(self.last_frame).as_secs_f64() * 1000.);
        self.last_frame = now;
        if let Some(cpu) = frame.info().cpu_usage {
            self.cpu.push(f64::from(cpu) * 1000.);
        }
        self.app.background_frame(ctx);
        self.app.window_logic(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.app.frame_ui(ui);
    }
    fn persist_egui_memory(&self) -> bool {
        false
    }
}

fn main() -> eframe::Result {
    let root = std::env::temp_dir().join(format!(
        "heya-zapfast-speed-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let (mut app, _) = App::headless(AppDirs::under(&root), Settings::default());
    app.backend.set_offline(true);
    assert!(app.backend.is_offline());
    app.link = LinkStatus::Connected;
    app.me = Some("1@s.whatsapp.net".into());
    app.account_mut().settings.auto_download = false;
    app.chats = (0..500)
        .map(|i| {
            let mut chat = Chat::new(
                format!("{}@s.whatsapp.net", i + 1),
                format!("Synthetic conversation {i}"),
            );
            chat.last_activity = 1791936000;
            chat.last = Some(LastMessage {
                from_me: false,
                sender: "6@s.whatsapp.net".into(),
                sender_name: None,
                summary: "Synthetic multiline preview 👩🏽‍💻 Second line".into(),
                full: String::new(),
                status: Delivery::None,
            });
            chat
        })
        .collect();
    app.open_chat = Some("6@s.whatsapp.net".into());
    app.conversations
        .insert("6@s.whatsapp.net".into(), conversation(100));
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default().with_inner_size([1200., 800.]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "ZapFast — synthetic UI speed",
        options,
        Box::new(move |cc| {
            app.attach(&cc.egui_ctx);
            let stop = Arc::new(AtomicBool::new(false));
            let done = stop.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    ctx.request_repaint();
                    std::thread::sleep(Duration::from_millis(16));
                }
            });
            Ok(Box::new(Bench {
                app,
                start: Instant::now(),
                phase: 0,
                last_frame: Instant::now(),
                last_tick: 0,
                frames: vec![],
                cpu: vec![],
                stop,
            }))
        }),
    );
    // This directory was created above for the detached synthetic app only.
    let _ = std::fs::remove_dir_all(root);
    result
}
