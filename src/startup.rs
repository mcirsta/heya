//! Bounded GUI startup diagnostics. No account data, polling thread or frame log.
use slint::{
    ComponentHandle,
    winit_031::{
        CustomApplicationHandler, EventResult, WinitWindowAccessor,
        winit::{event::WindowEvent, event_loop::ActiveEventLoop, window::WindowId},
    },
};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const LOG_LIMIT: u64 = 64 * 1024;
const RETAINED_LOGS: usize = 5;

struct Sink {
    file: File,
    bytes: u64,
}

struct Inner {
    started: Instant,
    sink: Mutex<Option<Sink>>,
    stage: Mutex<&'static str>,
    enabled: bool,
    first_redraw_returned: AtomicBool,
}

#[derive(Clone)]
pub struct Trace(Arc<Inner>);

impl Trace {
    pub fn start(path: Option<PathBuf>) -> Self {
        let path = path.or_else(default_log_path);
        let enabled = path.is_some();
        let sink = path.and_then(|path| match open_log(&path) {
            Ok(sink) => Some(sink),
            Err(_) => {
                eprintln!("Heya: startup log unavailable; continuing without a log file.");
                None
            }
        });
        Self(Arc::new(Inner {
            started: Instant::now(),
            sink: Mutex::new(sink),
            stage: Mutex::new("process_start"),
            enabled,
            first_redraw_returned: AtomicBool::new(false),
        }))
    }

    pub fn enabled(&self) -> bool {
        self.0.enabled
    }

    pub fn record(&self, stage: &'static str, detail: &str) {
        if !self.enabled() {
            return;
        }
        if let Ok(mut current) = self.0.stage.lock() {
            *current = stage;
        }
        let line = format!(
            "elapsed_ms={} stage={stage} {}\n",
            self.0.started.elapsed().as_millis(),
            single_line(detail)
        );
        if let Ok(mut guard) = self.0.sink.lock()
            && let Some(sink) = guard.as_mut()
            && sink.bytes + line.len() as u64 <= LOG_LIMIT
        {
            // File is unbuffered: a later crash/hang leaves the last checkpoint.
            if sink.file.write_all(line.as_bytes()).is_ok() {
                sink.bytes += line.len() as u64;
            }
        }
    }

    pub fn install_panic_hook(&self) {
        if !self.enabled() {
            return;
        }
        let previous = std::panic::take_hook();
        let trace = self.clone();
        std::panic::set_hook(Box::new(move |info| {
            let stage = trace.0.stage.try_lock().map_or("unknown", |stage| *stage);
            let location = info.location().map_or(String::new(), |location| {
                let source = Path::new(location.file())
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("unknown");
                format!(
                    " source={source} line={} column={}",
                    location.line(),
                    location.column()
                )
            });
            // Panic payloads can contain message text or identifiers. Never
            // copy them into this shareable GUI diagnostic log.
            trace.record(
                "panic",
                &format!("during={stage}{location} payload=omitted"),
            );
            previous(info);
        }));
    }

    pub fn first_redraw_returned(&self) -> bool {
        self.0.first_redraw_returned.load(Ordering::Relaxed)
    }
}

fn single_line(text: &str) -> String {
    text.chars()
        .take(768)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn open_log(path: &Path) -> io::Result<Sink> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("Startup trace requires a regular file"));
    }
    let bytes = metadata.len();
    Ok(Sink { file, bytes })
}

fn default_log_path() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let directory = directories::BaseDirs::new()?
        .home_dir()
        .join("Library/Logs/Heya");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&directory).ok()?;
    prune_logs(&directory);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    Some(directory.join(format!("startup-{stamp:020}-{}.log", std::process::id())))
}

fn is_startup_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("startup-"))
        .and_then(|name| name.strip_suffix(".log"))
        .and_then(|name| name.split_once('-'))
        .is_some_and(|(stamp, pid)| {
            !stamp.is_empty()
                && !pid.is_empty()
                && stamp.bytes().all(|c| c.is_ascii_digit())
                && pid.bytes().all(|c| c.is_ascii_digit())
        })
}

fn prune_logs(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut logs: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_startup_log(path))
        .collect();
    logs.sort();
    let excess = logs.len().saturating_sub(RETAINED_LOGS - 1);
    for path in logs.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn renderer_request(software: bool) -> &'static str {
    if software {
        "skia-software"
    } else if std::env::var_os("SLINT_BACKEND").is_some() {
        "environment-override"
    } else {
        "platform-default"
    }
}

fn may_retry(software: bool, explicit_backend: bool, stage: &str) -> bool {
    !software
        && !explicit_backend
        && matches!(stage, "backend_select" | "window_create" | "window_show")
}

/// A fresh process is necessary: Slint's global backend cannot be reset in place.
/// Only called before account services exist. An explicit backend is respected.
pub fn gui_step<T>(
    trace: &Trace,
    stage: &'static str,
    software: bool,
    init: impl FnOnce() -> Result<T, slint::PlatformError>,
) -> Result<T, slint::PlatformError> {
    trace.record(stage, "begin");
    let result = init();
    match &result {
        Ok(_) => trace.record(stage, "ok"),
        Err(error) => {
            trace.record(stage, &format!("failed error={error}"));
            let retry = may_retry(software, std::env::var_os("SLINT_BACKEND").is_some(), stage);
            if cfg!(target_os = "macos") && retry {
                retry_software(trace);
            }
        }
    }
    result
}

fn retry_software(trace: &Trace) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let Ok(executable) = std::env::current_exe() else {
            return;
        };
        trace.record(
            "software_retry",
            "replacing process; accounts not started; attempt=2/2",
        );
        let error = std::process::Command::new(executable)
            .args(std::env::args_os().skip(1))
            .arg("--software-rendering")
            .exec();
        trace.record("software_retry", &format!("exec_failed error={error}"));
    }
    #[cfg(not(unix))]
    let _ = trace;
}

pub fn select_backend(
    trace: &Trace,
    software: bool,
    check: bool,
) -> Result<(), slint::PlatformError> {
    if !trace.enabled() && !software && !check {
        return Ok(());
    }
    gui_step(trace, "backend_select", software, || {
        let mut selector = slint::BackendSelector::new();
        if software {
            selector = selector
                .backend_name("winit".into())
                .renderer_name("skia-software".into());
        }
        selector
            .with_winit_custom_application_handler(StartupEvents {
                trace: Some(trace.clone()),
                redraw_pending: false,
                resumed: false,
                native_window: false,
                check,
            })
            .select()
    })?;
    if trace.enabled() {
        install_surface_diagnostics(trace);
    }
    Ok(())
}

fn install_surface_diagnostics(trace: &Trace) {
    let trace = trace.clone();
    // Narrow adapter to the pinned Slint log API. Capture only its
    // surface-initialization failures, not general runtime/UI debug messages
    // which can contain private strings or image paths. Preserve stderr output.
    let _ = i_slint_core::context::with_global_context(
        || Err(slint::PlatformError::NoPlatform),
        |context| {
            context.set_log_message_handler(Some(Box::new(move |message| {
                use i_slint_core::debug_log::{LogMessageSource, default_log_message};
                if message.source() == LogMessageSource::Runtime && !trace.first_redraw_returned() {
                    let text = message.message_arguments().to_string();
                    if text.starts_with("Failed to initialize Skia ") {
                        trace.record("renderer_surface_fallback", &text);
                    }
                }
                default_log_message(message.message_arguments());
            })))
        },
    );
}

struct StartupEvents {
    trace: Option<Trace>,
    redraw_pending: bool,
    resumed: bool,
    native_window: bool,
    check: bool,
}

impl CustomApplicationHandler for StartupEvents {
    fn resumed(&mut self, _: &dyn ActiveEventLoop) -> EventResult {
        if !self.resumed {
            self.resumed = true;
            if let Some(trace) = &self.trace {
                trace.record("event_loop_resumed", "ok");
            }
        }
        EventResult::Propagate
    }

    fn window_event(
        &mut self,
        _: &dyn ActiveEventLoop,
        _: WindowId,
        native: Option<&dyn slint::winit_031::winit::window::Window>,
        _: Option<&slint::Window>,
        event: &WindowEvent,
    ) -> EventResult {
        let Some(trace) = &self.trace else {
            return EventResult::Propagate;
        };
        if let Some(window) = native {
            if !self.native_window {
                self.native_window = true;
                let size = window.surface_size();
                trace.record(
                    "native_window",
                    &format!(
                        "width={} height={} scale={} visible={:?} focused={} position={:?}",
                        size.width,
                        size.height,
                        window.scale_factor(),
                        window.is_visible(),
                        window.has_focus(),
                        window.outer_position().ok()
                    ),
                );
            }
            if matches!(event, WindowEvent::RedrawRequested) && !self.redraw_pending {
                self.redraw_pending = true;
                trace.record("first_redraw", "begin");
            }
        }
        EventResult::Propagate
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) -> EventResult {
        // Winit dispatches RedrawRequested synchronously. Reaching this point
        // without exiting establishes return from Slint's draw/present call,
        // not that the compositor displayed pixels on a physical monitor.
        if self.redraw_pending
            && !event_loop.exiting()
            && let Some(trace) = self.trace.take()
        {
            trace.0.first_redraw_returned.store(true, Ordering::Relaxed);
            trace.record("first_redraw_returned", "ok");
            if trace.enabled() {
                let _ = i_slint_core::context::with_global_context(
                    || Err(slint::PlatformError::NoPlatform),
                    |context| context.set_log_message_handler(None),
                );
            }
            if self.check {
                slint::Timer::single_shot(Duration::from_millis(250), || {
                    let _ = slint::quit_event_loop();
                });
            }
        }
        EventResult::Propagate
    }
}

pub fn observe_window(ui: &crate::AppWindow, trace: &Trace) {
    if !trace.enabled() {
        return;
    }
    // Finite main-loop callbacks, no watchdog and no thread kept alive. A stuck
    // native call cannot be interrupted here; its last checkpoint remains on disk.
    for (attempt, delay) in [(1, 200), (2, 1000)] {
        let weak = ui.as_weak();
        let trace = trace.clone();
        slint::Timer::single_shot(Duration::from_millis(delay), move || {
            if trace.first_redraw_returned() {
                return;
            }
            let Some(ui) = weak.upgrade() else { return };
            trace.record("event_loop_responsive", &format!("window_check={attempt}"));
            let found = ui
                .window()
                .with_winit_window(|window| {
                    // Do not undo an intentional minimize or steal focus after a
                    // successful first draw. Only repair an invisible startup window.
                    if window.is_visible() == Some(false) && window.is_minimized() != Some(true) {
                        trace.record("window_visibility_retry", &format!("attempt={attempt}/2"));
                        window.set_visible(true);
                        window.request_redraw();
                    }
                })
                .is_some();
            if !found {
                trace.record("native_window", "not_available_yet");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn panic_log_keeps_the_checkpoint_but_excludes_private_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panic.log");
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "startup::tests::startup_panic_child",
                "--ignored",
            ])
            .env("HEYA_TEST_STARTUP_PANIC_LOG", &path)
            .kill_on_drop(true);
        let result = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(!result.status.success());
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("stage=panic during=window_create"));
        assert!(text.contains("payload=omitted"));
        assert!(!text.contains("synthetic-private-payload"));
    }

    #[test]
    #[ignore = "only invoked by the isolated panic-log regression"]
    fn startup_panic_child() {
        let Some(path) = std::env::var_os("HEYA_TEST_STARTUP_PANIC_LOG") else {
            return;
        };
        let trace = Trace::start(Some(path.into()));
        trace.install_panic_hook();
        trace.record("window_create", "begin");
        panic!("synthetic-private-payload");
    }

    #[test]
    fn retry_is_bounded_and_only_precedes_account_startup() {
        for stage in ["backend_select", "window_create", "window_show"] {
            assert!(may_retry(false, false, stage));
            assert!(!may_retry(true, false, stage));
            assert!(!may_retry(false, true, stage));
        }
        for stage in [
            "preferences",
            "emoji_probe",
            "event_loop",
            "account_services",
            "shutdown",
        ] {
            assert!(!may_retry(false, false, stage));
        }
    }

    #[test]
    fn checkpoints_are_immediate_single_line_bounded_and_append_on_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.log");
        let trace = Trace::start(Some(path.clone()));
        trace.record("window_create", "failed\nforged_stage\r\0");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("stage=window_create"));
        assert!(!text.contains('\0'));
        Trace::start(Some(path.clone())).record("software_retry", "attempt=2/2");
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);
        for _ in 0..200 {
            trace.record("test", &"x".repeat(1000));
        }
        assert!(std::fs::metadata(&path).unwrap().len() <= LOG_LIMIT);
        assert!(single_line(&"x".repeat(1000)).len() <= 768);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn retention_only_removes_our_old_logs_and_never_follows_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..9 {
            std::fs::write(dir.path().join(format!("startup-{n:020}-123.log")), "").unwrap();
        }
        let other = dir.path().join("startup-not-ours.log");
        std::fs::write(&other, "keep").unwrap();
        prune_logs(dir.path());
        assert!(other.exists());
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            RETAINED_LOGS
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link.log");
            std::os::unix::fs::symlink(&other, &link).unwrap();
            assert!(open_log(&link).is_err());
            assert_eq!(std::fs::read_to_string(other).unwrap(), "keep");
        }
    }
}
