mod appearance;
mod archive;
mod attachment_worker;
mod attachments;
mod audio;
mod audio_devices;
mod audio_player;
mod audio_process;
mod avatars;
mod call_audio;
mod call_tones;
mod call_video;
mod calling;
#[cfg(test)]
mod calling_tests;
mod chat_management;
mod clipboard;
mod connection;
mod contact_info;
mod document_previews;
mod domain;
mod downloads;
mod downloads_file;
mod downloads_picker;
mod fixtures;
#[cfg(all(target_os = "linux", feature = "media-gstreamer"))]
mod gstreamer_call;
#[cfg(all(target_os = "linux", feature = "media-gstreamer"))]
mod gstreamer_call_video;
#[cfg(all(target_os = "linux", feature = "media-gstreamer"))]
mod gstreamer_memory;
#[cfg(all(target_os = "linux", feature = "media-gstreamer"))]
mod gstreamer_video;
#[cfg(target_os = "linux")]
mod linux_emoji;
mod live_presentation;
mod local_worker;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_audio;
#[cfg(target_os = "macos")]
mod macos_call_audio;
#[cfg(target_os = "macos")]
mod macos_call_video;
#[cfg(target_os = "macos")]
mod macos_video;
mod media_process;
mod message_actions;
mod message_content;
mod message_navigation;
mod message_thumbnails;
mod message_time;
mod messaging;
mod pdf_preview;
mod performance;
mod phone_history;
mod photo_albums;
mod photos;
mod preferences;
mod presence;
mod presentation;
mod presentation_actions;
mod presentation_attachments;
mod presentation_audio;
mod presentation_avatars;
mod presentation_calling;
mod presentation_clipboard;
mod presentation_contact_info;
mod presentation_content;
mod presentation_downloads;
mod presentation_emoji;
#[cfg(test)]
mod presentation_emoji_tests;
#[cfg(test)]
mod presentation_hover_tests;
#[cfg(test)]
mod presentation_menu_tests;
mod presentation_message_controls;
mod presentation_navigation;
mod presentation_photos;
mod presentation_preferences;
mod presentation_presence;
#[cfg(test)]
mod presentation_read_tests;
mod presentation_scroll;
mod presentation_settings_io;
mod presentation_storage;
mod presentation_text;
mod presentation_video;
mod presentation_voice;
mod protocol;
mod protocol_actions;
mod protocol_audio;
mod protocol_avatars;
mod protocol_business;
mod protocol_call_video;
mod protocol_calling;
mod protocol_calls;
mod protocol_chat_management;
mod protocol_content;
mod protocol_document_previews;
mod protocol_downloads;
mod protocol_edits;
mod protocol_history;
mod protocol_media;
mod protocol_messages;
mod protocol_outbox;
mod protocol_photos;
mod protocol_presence;
mod protocol_reads;
mod protocol_receipts;
mod protocol_sync;
mod protocol_uploads;
mod protocol_video;
mod service;
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod skia_cpu_tests;
mod startup;
mod storage;
mod taskbar_badge;
mod video;
mod video_process;
mod voice;
mod voice_recorder;

use slint::ComponentHandle;
use std::{path::PathBuf, time::Duration};

// The toolkit generates an unused component-embedding stub containing todo!.
// Keep this exception limited to generated code.
#[allow(clippy::todo)]
mod ui {
    slint::include_modules!();
}
use ui::{AppWindow, ChatRow, MessageActionRow, MessageRow, Theme, TimelineRow};

#[derive(Default)]
struct Options {
    demo: bool,
    offline: bool,
    theme: Option<String>,
    screenshot: Option<PathBuf>,
    ui_benchmark: bool,
    demo_chat: Option<i32>,
    size: Option<(u32, u32)>,
    startup_trace: Option<PathBuf>,
    software_rendering: bool,
    startup_check: bool,
    change_storage: Option<storage::Mode>,
}

fn options_from(args: impl IntoIterator<Item = String>) -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => options.demo = true,
            "--offline" => options.offline = true,
            "--light" => options.theme = Some("Light".into()),
            "--dark" => options.theme = Some("Dark".into()),
            "--ui-benchmark" => options.ui_benchmark = true,
            "--software-rendering" => options.software_rendering = true,
            "--startup-check" => options.startup_check = true,
            "--change-storage" => {
                options.change_storage = Some(match args.next().as_deref() {
                    Some("encrypted") => storage::Mode::Encrypted,
                    Some("unencrypted") => storage::Mode::Unencrypted,
                    _ => return Err("--change-storage needs encrypted or unencrypted".into()),
                });
            }
            "--startup-trace" => {
                options.startup_trace = Some(
                    args.next()
                        .ok_or("--startup-trace needs a log path")?
                        .into(),
                );
            }
            "--demo-chat" => {
                options.demo_chat = Some(
                    args.next()
                        .ok_or("--demo-chat needs a synthetic chat ID")?
                        .parse()
                        .map_err(|_| "Invalid demo chat ID")?,
                );
            }
            "--screenshot" => {
                options.screenshot =
                    Some(args.next().ok_or("--screenshot needs a PNG path")?.into())
            }
            "--size" => {
                let value = args.next().ok_or("--size needs WIDTHxHEIGHT")?;
                let (width, height) = value.split_once('x').ok_or("--size needs WIDTHxHEIGHT")?;
                let width: u32 = width.parse().map_err(|_| "Invalid width")?;
                let height: u32 = height.parse().map_err(|_| "Invalid height")?;
                if !(520..=3840).contains(&width) || !(480..=2160).contains(&height) {
                    return Err("Size must be within 520x480 and 3840x2160".into());
                }
                options.size = Some((width, height));
            }
            "--help" | "-h" => {
                println!(
                    "Heya\n\nUsage: heya [--demo | --offline] [--light | --dark] [--size WIDTHxHEIGHT] [--demo-chat ID] [--screenshot FILE.png] [--ui-benchmark] [--software-rendering] [--startup-trace FILE.log] [--startup-check]\n\nSaved sessions resume automatically unless disabled in Settings → Account. On first use, click Connect to link WhatsApp.\n--offline opens saved chats without connecting, regardless of that setting; encrypted storage still requires the OS key store. Connect remains available.\n--demo uses synthetic chats without opening account files or the network.\n--demo-chat selects a synthetic chat (1–16); 7 demonstrates message cards; 8 dates and spacing; 9 received photos; 10 received audio; 11 received documents; 12 pin/unpin notices; 13 color emoji; 14 received videos; 15 call history; 16 an empty conversation.\n--screenshot requires --demo and exits after capturing the synthetic UI.\n--ui-benchmark requires --demo, measures synthetic updates/resizing and exits; it cannot be combined with --screenshot.\n--software-rendering uses Skia's CPU renderer for GUI troubleshooting.\n--startup-trace writes bounded GUI checkpoints, without account content. macOS logs startup automatically under ~/Library/Logs/Heya.\n--startup-check requires --demo and exits after the first native redraw returns; do not combine it with screenshots or benchmarks."
                );
                return Ok(None);
            }
            other => return Err(format!("Unknown option {other}. Use --help.")),
        }
    }
    if options.demo && options.change_storage.is_some() {
        return Err("Storage conversion cannot be combined with --demo".into());
    }
    if options.demo_chat.is_some() && (!options.demo || !matches!(options.demo_chat, Some(1..=16)))
    {
        return Err("--demo-chat requires --demo and a chat ID from 1 to 16".into());
    }
    if options.screenshot.is_some() && !options.demo {
        return Err(
            "--screenshot requires --demo; live pairing and account data cannot be captured".into(),
        );
    }
    if options.ui_benchmark && (!options.demo || options.screenshot.is_some()) {
        return Err(
            "--ui-benchmark requires --demo and cannot be combined with --screenshot".into(),
        );
    }
    if options.startup_check
        && (!options.demo || options.screenshot.is_some() || options.ui_benchmark)
    {
        return Err("--startup-check requires --demo and cannot be combined with --screenshot or --ui-benchmark".into());
    }
    Ok(Some(options))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--heya-video-worker") {
        std::process::exit(if video_process::child().is_ok() { 0 } else { 1 });
    }
    #[cfg(target_os = "macos")]
    if std::env::args().nth(1).as_deref() == Some("--heya-pdf-worker") {
        macos::pdf_worker();
    }
    if std::env::args().nth(1).as_deref() == Some("--heya-call-audio-worker") {
        call_audio::child_main();
    }
    if std::env::args().nth(1).as_deref() == Some("--heya-call-video-worker") {
        call_video::child_main();
    }
    if std::env::args().nth(1).as_deref() == Some("--heya-audio-worker") {
        audio_process::child_main();
    }
    // A short-lived media helper must never initialize the UI or account services.
    if std::env::args().nth(1).as_deref() == Some("--heya-media-worker") {
        media_process::child_main(&std::env::args().nth(2).unwrap_or_default());
    }
    let Some(options) = options_from(std::env::args().skip(1)).map_err(std::io::Error::other)?
    else {
        return Ok(());
    };
    let trace = startup::Trace::start(options.startup_trace.clone());
    trace.install_panic_hook();
    trace.record(
        "process_start",
        &format!(
            "version={} build={} os={} arch={} pid={} mode={} renderer_request={}",
            env!("CARGO_PKG_VERSION"),
            option_env!("HEYA_BUILD_NUMBER").unwrap_or("development"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::process::id(),
            if options.demo {
                "demo"
            } else if options.offline {
                "offline"
            } else {
                "normal"
            },
            startup::renderer_request(options.software_rendering)
        ),
    );
    #[cfg(target_os = "macos")]
    trace.record("system", &macos::startup_system_info());
    let result = run(options, &trace);
    trace.record(
        "process_exit",
        if result.is_ok() {
            "ok"
        } else {
            "failed; see last checkpoint"
        },
    );
    result
}

fn run(options: Options, trace: &startup::Trace) -> Result<(), Box<dyn std::error::Error>> {
    startup::select_backend(trace, options.software_rendering, options.startup_check)?;
    trace.record("runtime_create", "begin");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    trace.record("runtime_create", "ok");
    let demo = options.demo;
    let worker_trace = trace.clone();
    let (preferences, wallpaper, storage_status) =
        runtime.block_on(runtime.spawn_blocking(move || {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                worker_trace.record("emoji_probe", "begin");
                #[cfg(target_os = "macos")]
                macos::prepare_emoji_support();
                #[cfg(target_os = "linux")]
                linux_emoji::prepare_emoji_support();
                worker_trace.record("emoji_probe", "ok");
            }
            worker_trace.record("preferences_load", "begin");
            let preferences = presentation_preferences::prepare(demo);
            worker_trace.record("preferences_load", "ok");
            worker_trace.record("wallpaper_decode", "begin");
            let wallpaper = presentation::wallpaper_pixels();
            worker_trace.record(
                "wallpaper_decode",
                if wallpaper.is_ok() { "ok" } else { "failed" },
            );
            (preferences, wallpaper, (!demo).then(storage::status))
        }))?;
    let (mut preferences, preferences_path, preferences_status) = preferences;
    let wallpaper = wallpaper?;
    if let Some(theme) = options.theme {
        preferences.theme = theme;
    }
    let ui = startup::gui_step(
        trace,
        "window_create",
        options.software_rendering,
        AppWindow::new,
    )?;
    // Set this after backend initialization, before creating the native window.
    // Match the desktop entry so Wayland/X11 taskbars resolve Heya's icon.
    #[cfg(target_os = "linux")]
    slint::set_xdg_app_id("org.heya.Heya")?;
    presentation_scroll::bind(&ui);
    let _wheel_trace = (options.demo && std::env::var_os("HEYA_TEST_WHEEL_TRACE").is_some())
        .then(|| presentation_scroll::trace(&ui));
    ui.global::<Theme>()
        .set_wallpaper_image(slint::Image::from_rgba8(
            slint::SharedPixelBuffer::clone_from_slice(
                wallpaper.as_raw(),
                wallpaper.width(),
                wallpaper.height(),
            ),
        ));
    drop(wallpaper);
    if let Some((width, height)) = options.size {
        ui.window()
            .set_size(slint::LogicalSize::new(width as f32, height as f32));
    }
    ui.set_demo_mode(options.demo);
    // On macOS, retryable GUI setup precedes account services and their lock.
    // Other platforms retain their existing show/attachment ordering.
    #[cfg(target_os = "macos")]
    startup::gui_step(trace, "window_show", options.software_rendering, || {
        ui.show()
    })?;
    trace.record("presentation_attach", "begin");
    let (preference_saver, settings_operations, current_preferences) =
        presentation_preferences::attach(
            &ui,
            &runtime,
            preferences,
            preferences_path,
            preferences_status,
        );
    presentation_content::bind_actions(&ui, &runtime);
    let (storage_restart, storage_task) = presentation_storage::attach(
        &ui,
        &runtime,
        storage_status,
        options.change_storage.is_some(),
    );
    if let Some(mode) = options.change_storage {
        ui.set_storage_target(mode == storage::Mode::Encrypted);
    }
    let service = if options.demo
        && options.ui_benchmark
        && (std::env::var_os("HEYA_BENCHMARK_ALBUM").is_some()
            || std::env::var_os("HEYA_BENCHMARK_CONTACT_INFO").is_some())
    {
        // The archive-backed viewer benchmark supplies its own model and photo
        // publisher. A second demo publisher would race its startup snapshot.
        runtime.spawn(async {})
    } else if options.demo {
        presentation::attach(&ui, &runtime, current_preferences.clone())
    } else {
        live_presentation::attach(
            &ui,
            &runtime,
            options.offline,
            current_preferences.clone(),
            options.change_storage,
        )
    };
    trace.record("presentation_attach", "ok");
    if let Some(id) = options.demo_chat {
        ui.invoke_select_chat(id);
        ui.set_show_conversation(true);
    }
    let benchmark = options
        .ui_benchmark
        .then(|| presentation::benchmark::start(&ui, &runtime))
        .transpose()?;
    #[cfg(not(target_os = "macos"))]
    startup::gui_step(trace, "window_show", options.software_rendering, || {
        ui.show()
    })?;
    startup::observe_window(&ui, trace);

    let capture_error = std::rc::Rc::new(std::cell::RefCell::new(None));
    if let Some(path) = options.screenshot {
        let ui = ui.as_weak();
        let error = capture_error.clone();
        slint::Timer::single_shot(Duration::from_millis(700), move || {
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                let ui = ui.upgrade().ok_or("Window closed before capture")?;
                let buffer = ui.window().take_snapshot()?;
                let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
                let mut encoder = png::Encoder::new(file, buffer.width(), buffer.height());
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()?
                    .write_image_data(buffer.as_bytes())?;
                println!(
                    "Saved {} ({}×{} pixels)",
                    path.display(),
                    buffer.width(),
                    buffer.height()
                );
                println!(
                    "Timeline: offset={}, content={}, visible={}",
                    ui.get_timeline_y(),
                    ui.get_timeline_height(),
                    ui.get_timeline_visible()
                );
                Ok(())
            })();
            if let Err(cause) = result {
                *error.borrow_mut() = Some(cause.to_string());
            }
            let _ = slint::quit_event_loop();
        });
    }

    trace.record("event_loop", "enter");
    let result = slint::run_event_loop();
    match &result {
        Ok(()) => trace.record("event_loop", "returned"),
        Err(error) => trace.record("event_loop", &format!("failed error={error}")),
    }
    let shutdown_started = std::time::Instant::now();
    let benchmark_failed = benchmark
        .as_ref()
        .is_some_and(|benchmark| benchmark.failed());
    drop(benchmark);
    let hidden = ui.hide();
    drop(ui); // Releases all command senders before joining the service.
    runtime.block_on(async {
        let _ = storage_task.await;
        let mut settings_operations=settings_operations;
        if tokio::time::timeout(Duration::from_secs(5), &mut settings_operations).await.is_err(){settings_operations.abort();let _=settings_operations.await;}
        if let Some(mut saver) = preference_saver
            && tokio::time::timeout(Duration::from_secs(5), &mut saver).await.is_err()
        {
            saver.abort();
            eprintln!("Heya's settings save did not finish before shutdown.");
        }
        let mut service = service;
        if tokio::time::timeout(Duration::from_secs(20), &mut service)
            .await
            .is_err()
        {
            service.abort();
            let _ = service.await;
            eprintln!("Heya's background service did not stop within 20 seconds; shutdown was interrupted.");
        }
    });
    runtime.shutdown_timeout(Duration::from_secs(2));
    trace.record(
        "shutdown_finished",
        &format!("elapsed_ms={}", shutdown_started.elapsed().as_millis()),
    );
    eprintln!(
        "Heya shutdown: finished in {} ms.",
        shutdown_started.elapsed().as_millis()
    );
    result?;
    hidden?;
    if options.startup_check && !trace.first_redraw_returned() {
        return Err(
            std::io::Error::other("Startup check did not reach a completed native redraw").into(),
        );
    }
    if benchmark_failed {
        return Err(std::io::Error::other(
            "The synthetic archive benchmark failed; see its diagnostics.",
        )
        .into());
    }
    if let Some(error) = capture_error.borrow_mut().take() {
        return Err(std::io::Error::other(error).into());
    }
    if let Some(mode) = storage_restart.0.get() {
        let mut restart = std::process::Command::new(std::env::current_exe()?);
        restart.arg("--change-storage").arg(match mode {
            storage::Mode::Encrypted => "encrypted",
            storage::Mode::Unencrypted => "unencrypted",
        });
        if options.offline {
            restart.arg("--offline");
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            return Err(restart.exec().into());
        }
        #[cfg(not(unix))]
        restart.spawn()?;
    }
    Ok(())
}

#[cfg(test)]
mod option_tests {
    use super::*;

    #[test]
    fn live_is_default_and_only_synthetic_screenshots_are_allowed() {
        assert!(!options_from(Vec::new()).unwrap().unwrap().demo);
        assert!(
            options_from(["--demo", "--change-storage", "unencrypted"].map(String::from)).is_err()
        );
        assert!(options_from(["--change-storage", "invalid"].map(String::from)).is_err());
        assert!(options_from(["--screenshot", "/tmp/heya-live.png"].map(String::from)).is_err());
        let options =
            options_from(["--demo", "--screenshot", "/tmp/heya-synthetic.png"].map(String::from))
                .unwrap()
                .unwrap();
        assert!(options.demo);
        assert!(options.screenshot.is_some());
        assert!(options_from(["--ui-benchmark"].map(String::from)).is_err());
        assert!(options_from(["--demo-chat", "7"].map(String::from)).is_err());
        assert!(options_from(["--demo", "--demo-chat", "17"].map(String::from)).is_err());
        assert_eq!(
            options_from(["--demo", "--demo-chat", "7"].map(String::from))
                .unwrap()
                .unwrap()
                .demo_chat,
            Some(7)
        );
        assert!(
            options_from(
                ["--demo", "--ui-benchmark", "--screenshot", "/tmp/test.png"].map(String::from)
            )
            .is_err()
        );
        assert!(
            options_from(["--demo", "--ui-benchmark"].map(String::from))
                .unwrap()
                .unwrap()
                .ui_benchmark
        );
        assert!(options_from(["--startup-check"].map(String::from)).is_err());
        assert!(
            options_from(["--demo", "--startup-check", "--ui-benchmark"].map(String::from))
                .is_err()
        );
        assert!(options_from(["--startup-trace"].map(String::from)).is_err());
        let startup = options_from(
            [
                "--demo",
                "--startup-check",
                "--software-rendering",
                "--startup-trace",
                "/tmp/startup.log",
            ]
            .map(String::from),
        )
        .unwrap()
        .unwrap();
        assert!(startup.startup_check && startup.software_rendering);
        assert_eq!(
            startup.startup_trace,
            Some(PathBuf::from("/tmp/startup.log"))
        );
    }
}
