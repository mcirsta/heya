mod appearance;
mod archive;
mod attachment_worker;
mod attachments;
mod audio;
mod audio_devices;
mod audio_player;
mod avatars;
mod connection;
mod document_previews;
mod domain;
mod downloads;
mod downloads_file;
mod downloads_picker;
mod fixtures;
mod live_presentation;
mod local_worker;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_audio;
mod media_process;
mod message_actions;
mod message_content;
mod message_thumbnails;
mod message_time;
mod messaging;
mod pdf_preview;
mod performance;
mod phone_history;
mod photos;
mod preferences;
mod presence;
mod presentation;
mod presentation_actions;
mod presentation_attachments;
mod presentation_audio;
mod presentation_avatars;
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
mod presentation_photos;
mod presentation_preferences;
mod presentation_presence;
#[cfg(test)]
mod presentation_read_tests;
mod presentation_scroll;
mod presentation_settings_io;
mod presentation_text;
mod presentation_voice;
mod protocol;
mod protocol_actions;
mod protocol_audio;
mod protocol_avatars;
mod protocol_business;
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
mod protocol_sync;
mod protocol_uploads;
mod service;
mod storage;
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
                    "Heya\n\nUsage: heya [--demo | --offline] [--light | --dark] [--size WIDTHxHEIGHT] [--demo-chat ID] [--screenshot FILE.png] [--ui-benchmark]\n\nSaved sessions resume automatically. On first use, click Connect to link WhatsApp.\n--offline opens saved chats without connecting; Connect remains available.\n--demo uses synthetic chats without opening account files or the network.\n--demo-chat selects a synthetic chat (1–13); 7 demonstrates message cards; 8 dates and spacing; 9 received photos; 10 received audio; 11 received documents; 12 pin/unpin notices; 13 color emoji.\n--screenshot requires --demo and exits after capturing the synthetic UI.\n--ui-benchmark requires --demo, measures synthetic updates/resizing and exits; it cannot be combined with --screenshot."
                );
                return Ok(None);
            }
            other => return Err(format!("Unknown option {other}. Use --help.")),
        }
    }
    if options.demo_chat.is_some() && (!options.demo || !matches!(options.demo_chat, Some(1..=13)))
    {
        return Err("--demo-chat requires --demo and a chat ID from 1 to 13".into());
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
    Ok(Some(options))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    if std::env::args().nth(1).as_deref() == Some("--heya-pdf-worker") {
        macos::pdf_worker();
    }
    // A short-lived media helper must never initialize the UI or account services.
    if std::env::args().nth(1).as_deref() == Some("--heya-media-worker") {
        media_process::child_main(&std::env::args().nth(2).unwrap_or_default());
    }
    let Some(options) = options_from(std::env::args().skip(1)).map_err(std::io::Error::other)?
    else {
        return Ok(());
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let demo = options.demo;
    let (preferences, wallpaper) = runtime.block_on(runtime.spawn_blocking(move || {
        (
            presentation_preferences::prepare(demo),
            presentation::wallpaper_pixels(),
        )
    }))?;
    let (mut preferences, preferences_path, preferences_status) = preferences;
    let wallpaper = wallpaper?;
    if let Some(theme) = options.theme {
        preferences.theme = theme;
    }
    let ui = AppWindow::new()?;
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
    let (preference_saver, settings_operations, current_preferences) =
        presentation_preferences::attach(
            &ui,
            &runtime,
            preferences,
            preferences_path,
            preferences_status,
        );
    presentation_content::bind_actions(&ui, &runtime);
    let service = if options.demo {
        presentation::attach(&ui, &runtime, current_preferences.clone())
    } else {
        live_presentation::attach(&ui, &runtime, options.offline, current_preferences.clone())
    };
    if let Some(id) = options.demo_chat {
        ui.invoke_select_chat(id);
        ui.set_show_conversation(true);
    }
    let benchmark = options
        .ui_benchmark
        .then(|| presentation::benchmark::start(&ui, &runtime))
        .transpose()?;
    ui.show()?;

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

    let result = slint::run_event_loop();
    let shutdown_started = std::time::Instant::now();
    let benchmark_failed = benchmark
        .as_ref()
        .is_some_and(|benchmark| benchmark.failed());
    drop(benchmark);
    let hidden = ui.hide();
    drop(ui); // Releases all command senders before joining the service.
    runtime.block_on(async {
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
    eprintln!(
        "Heya shutdown: finished in {} ms.",
        shutdown_started.elapsed().as_millis()
    );
    result?;
    hidden?;
    if benchmark_failed {
        return Err(std::io::Error::other(
            "The synthetic archive benchmark failed; see its diagnostics.",
        )
        .into());
    }
    if let Some(error) = capture_error.borrow_mut().take() {
        return Err(std::io::Error::other(error).into());
    }
    Ok(())
}

#[cfg(test)]
mod option_tests {
    use super::*;

    #[test]
    fn live_is_default_and_only_synthetic_screenshots_are_allowed() {
        assert!(!options_from(Vec::new()).unwrap().unwrap().demo);
        assert!(options_from(["--screenshot", "/tmp/heya-live.png"].map(String::from)).is_err());
        let options =
            options_from(["--demo", "--screenshot", "/tmp/heya-synthetic.png"].map(String::from))
                .unwrap()
                .unwrap();
        assert!(options.demo);
        assert!(options.screenshot.is_some());
        assert!(options_from(["--ui-benchmark"].map(String::from)).is_err());
        assert!(options_from(["--demo-chat", "7"].map(String::from)).is_err());
        assert!(options_from(["--demo", "--demo-chat", "14"].map(String::from)).is_err());
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
    }
}
